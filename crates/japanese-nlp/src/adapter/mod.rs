use std::{
    borrow::Cow,
    collections::{BTreeMap, VecDeque},
};

use contracts::{AppErrorV1, TokenId, TokenV1, error_codes};
use lindera::{dictionary::load_dictionary, mode::Mode, segmenter::Segmenter};
use parking_lot::Mutex;
use ports::TokenizerPort;
use sha2::{Digest, Sha256};

const TOKENIZER_VERSION: &str = "lindera-6.0.0-ipadic";
const MAX_SENTENCE_BYTES: usize = 64 * 1024;
const MAX_CACHE_ENTRIES: usize = 512;
const MAX_CACHE_WEIGHT: usize = 4 * 1024 * 1024;

pub struct LinderaTokenizer {
    state: Mutex<TokenizerState>,
}

struct TokenizerState {
    segmenter: Segmenter,
    cache: BTreeMap<String, CachedTokens>,
    recency: VecDeque<String>,
    cache_weight: usize,
}

struct CachedTokens {
    tokens: Vec<TokenV1>,
    weight: usize,
}

impl std::fmt::Debug for LinderaTokenizer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LinderaTokenizer")
            .field("version", &TOKENIZER_VERSION)
            .finish_non_exhaustive()
    }
}

impl LinderaTokenizer {
    pub fn embedded_ipadic() -> Result<Self, AppErrorV1> {
        let dictionary = load_dictionary("embedded://ipadic").map_err(tokenizer_error)?;
        Ok(Self {
            state: Mutex::new(TokenizerState {
                segmenter: Segmenter::new(Mode::Normal, dictionary, None),
                cache: BTreeMap::new(),
                recency: VecDeque::new(),
                cache_weight: 0,
            }),
        })
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.state.lock().cache.len()
    }
}

impl TokenizerPort for LinderaTokenizer {
    fn tokenize(&self, sentence: &str) -> Result<Vec<TokenV1>, AppErrorV1> {
        if sentence.len() > MAX_SENTENCE_BYTES {
            return Err(AppErrorV1::new(
                error_codes::INVALID_REQUEST,
                "Subtitle text exceeds the tokenizer safety limit.",
                false,
            ));
        }
        let mut state = self.state.lock();
        if let Some(tokens) = state
            .cache
            .get(sentence)
            .map(|cached| cached.tokens.clone())
        {
            state.touch(sentence);
            return Ok(tokens);
        }
        let mut tokens = state
            .segmenter
            .segment(Cow::Borrowed(sentence))
            .map_err(tokenizer_error)?;
        let mut result = Vec::with_capacity(tokens.len());
        let mut cursor = 0_usize;
        for token in &mut tokens {
            let surface = token.surface.to_string();
            let byte_start = token.byte_start;
            let byte_end = token.byte_end;
            if byte_start < cursor
                || byte_end < byte_start
                || sentence.get(byte_start..byte_end) != Some(surface.as_str())
            {
                return Err(tokenizer_error(
                    "Tokenizer returned overlapping or invalid UTF-8 spans.",
                ));
            }
            if byte_start > cursor {
                result.push(passthrough_token(
                    sentence,
                    cursor,
                    byte_start,
                    result.len(),
                )?);
            }
            let details = token.details();
            let unknown = details.first().is_none_or(|value| *value == "UNK");
            let part_of_speech = if unknown {
                vec![classify_unknown(&surface).into()]
            } else {
                let mut values = Vec::with_capacity(5);
                values.push(normalize_part_of_speech(details.first().copied()).into());
                values.extend(
                    details
                        .iter()
                        .take(4)
                        .filter(|value| **value != "*")
                        .map(|value| (*value).to_owned()),
                );
                values
            };
            let lemma = details
                .get(6)
                .filter(|value| **value != "*")
                .map_or_else(|| surface.clone(), |value| (*value).to_owned());
            let reading = details.get(7).filter(|value| **value != "*").map_or_else(
                || katakana_to_hiragana(&surface),
                |value| katakana_to_hiragana(value),
            );
            let pronunciation = details
                .get(8)
                .filter(|value| **value != "*")
                .map(|value| (*value).to_owned());
            let lookup_candidate =
                surface.chars().any(is_lookup_character) && !is_punctuation_only(&surface);
            result.push(TokenV1 {
                token_id: token_id(sentence, result.len(), byte_start, byte_end),
                surface,
                byte_start: u32::try_from(byte_start)
                    .map_err(|_| tokenizer_error("token byte start overflow"))?,
                byte_end: u32::try_from(byte_end)
                    .map_err(|_| tokenizer_error("token byte end overflow"))?,
                lemma,
                reading,
                pronunciation,
                part_of_speech,
                lookup_candidate,
            });
            cursor = byte_end;
        }
        if cursor < sentence.len() {
            result.push(passthrough_token(
                sentence,
                cursor,
                sentence.len(),
                result.len(),
            )?);
        }
        verify_spans(sentence, &result)?;
        state.insert_cache(sentence, result.clone());
        Ok(result)
    }

    fn version(&self) -> &str {
        TOKENIZER_VERSION
    }
}

fn passthrough_token(
    sentence: &str,
    start: usize,
    end: usize,
    position: usize,
) -> Result<TokenV1, AppErrorV1> {
    let surface = sentence
        .get(start..end)
        .ok_or_else(|| tokenizer_error("Tokenizer omitted a non-UTF-8-aligned span."))?
        .to_owned();
    Ok(TokenV1 {
        token_id: token_id(sentence, position, start, end),
        surface: surface.clone(),
        byte_start: u32::try_from(start)
            .map_err(|_| tokenizer_error("token byte start overflow"))?,
        byte_end: u32::try_from(end).map_err(|_| tokenizer_error("token byte end overflow"))?,
        lemma: surface.clone(),
        reading: surface,
        pronunciation: None,
        part_of_speech: vec!["separator".into()],
        lookup_candidate: false,
    })
}

impl TokenizerState {
    fn touch(&mut self, sentence: &str) {
        if let Some(index) = self.recency.iter().position(|value| value == sentence) {
            self.recency.remove(index);
        }
        self.recency.push_back(sentence.to_owned());
    }

    fn insert_cache(&mut self, sentence: &str, tokens: Vec<TokenV1>) {
        let weight = sentence.len()
            + tokens
                .iter()
                .map(|token| {
                    token.surface.len()
                        + token.lemma.len()
                        + token.reading.len()
                        + token.pronunciation.as_ref().map_or(0, String::len)
                        + token.part_of_speech.iter().map(String::len).sum::<usize>()
                        + 64
                })
                .sum::<usize>();
        if weight > MAX_CACHE_WEIGHT {
            return;
        }
        while self.cache.len() >= MAX_CACHE_ENTRIES
            || self.cache_weight.saturating_add(weight) > MAX_CACHE_WEIGHT
        {
            let Some(oldest) = self.recency.pop_front() else {
                break;
            };
            if let Some(removed) = self.cache.remove(&oldest) {
                self.cache_weight = self.cache_weight.saturating_sub(removed.weight);
            }
        }
        self.cache
            .insert(sentence.to_owned(), CachedTokens { tokens, weight });
        self.cache_weight = self.cache_weight.saturating_add(weight);
        self.touch(sentence);
    }
}

fn token_id(sentence: &str, position: usize, start: usize, end: usize) -> TokenId {
    let mut hasher = Sha256::new();
    hasher.update(b"migaku-token-v1\0");
    hasher.update(TOKENIZER_VERSION.as_bytes());
    hasher.update(sentence.as_bytes());
    hasher.update(u64::try_from(position).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(u64::try_from(start).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(u64::try_from(end).unwrap_or(u64::MAX).to_le_bytes());
    let digest = hex::encode(hasher.finalize());
    TokenId::new(format!("tok_{}", &digest[..24]))
}

fn verify_spans(sentence: &str, tokens: &[TokenV1]) -> Result<(), AppErrorV1> {
    let mut cursor = 0_usize;
    for token in tokens {
        let start = token.byte_start as usize;
        let end = token.byte_end as usize;
        if start != cursor
            || end < start
            || sentence.get(start..end) != Some(token.surface.as_str())
        {
            return Err(tokenizer_error(
                "Tokenizer returned non-contiguous or invalid UTF-8 spans.",
            ));
        }
        cursor = end;
    }
    if cursor != sentence.len() {
        return Err(tokenizer_error(
            "Tokenizer did not preserve the complete subtitle text.",
        ));
    }
    Ok(())
}

fn is_lookup_character(character: char) -> bool {
    matches!(character,
        '\u{3040}'..='\u{30ff}' |
        '\u{3400}'..='\u{4dbf}' |
        '\u{4e00}'..='\u{9fff}' |
        '\u{f900}'..='\u{faff}'
    ) || character.is_alphanumeric()
}

fn is_punctuation_only(value: &str) -> bool {
    value.chars().all(|character| {
        character.is_whitespace()
            || "。、！？…・「」『』（）()［］[]【】〈〉《》ー〜～,.!?;:".contains(character)
    })
}

fn classify_unknown(value: &str) -> &'static str {
    if is_punctuation_only(value) {
        "punctuation"
    } else {
        "unknown"
    }
}

fn normalize_part_of_speech(value: Option<&str>) -> &'static str {
    match value {
        Some("名詞") => "noun",
        Some("動詞") => "verb",
        Some("形容詞") => "adjective",
        Some("副詞") => "adverb",
        Some("助動詞") => "auxiliary",
        Some("助詞") => "particle",
        Some("記号") => "symbol",
        Some("連体詞") => "prenominal",
        Some("接続詞") => "conjunction",
        Some("感動詞") => "interjection",
        Some("接頭詞") => "prefix",
        Some("フィラー") => "filler",
        Some("その他") => "other",
        _ => "unknown",
    }
}

fn katakana_to_hiragana(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\u{30a1}'..='\u{30f6}' => char::from_u32(character as u32 - 0x60).unwrap_or(character),
            _ => character,
        })
        .collect()
}

fn tokenizer_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::DICTIONARY_UNAVAILABLE,
        "The Japanese tokenizer is unavailable. Repair or reinstall its dictionary.",
        true,
    )
    .with_diagnostics(error.to_string())
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "private-corpus-audit")]
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    #[cfg(feature = "private-corpus-audit")]
    const SUBTITLE_AUDIT_SAMPLE_SIZE: usize = 40;
    #[cfg(feature = "private-corpus-audit")]
    const SUBTITLE_AUDIT_MIN_ANNOTATIONS: usize = 120;
    #[cfg(feature = "private-corpus-audit")]
    const SUBTITLE_AUDIT_THRESHOLD_PER_MILLE: usize = 900;

    #[cfg(feature = "private-corpus-audit")]
    #[derive(Debug)]
    struct GoldAnnotation<'a> {
        cue_id: u32,
        surface: &'a str,
        occurrence: usize,
        lemma: &'a str,
        part_of_speech: &'a str,
        reading: &'a str,
    }

    #[cfg(feature = "private-corpus-audit")]
    #[derive(Default)]
    struct AuditMetric {
        passed: usize,
        total: usize,
    }

    #[cfg(feature = "private-corpus-audit")]
    impl AuditMetric {
        fn record(&mut self, passed: bool) {
            self.total += 1;
            self.passed += usize::from(passed);
        }

        fn percentage(&self) -> f64 {
            self.passed as f64 * 100.0 / self.total as f64
        }

        fn meets_threshold(&self) -> bool {
            self.passed * 1_000 >= self.total * SUBTITLE_AUDIT_THRESHOLD_PER_MILLE
        }
    }

    #[test]
    fn tokenizes_conjugation_and_preserves_exact_surface() -> Result<(), AppErrorV1> {
        let tokenizer = LinderaTokenizer::embedded_ipadic()?;
        let sentence = "昨日は映画を見ました。";
        let tokens = tokenizer.tokenize(sentence)?;
        assert_eq!(
            tokens
                .iter()
                .map(|token| token.surface.as_str())
                .collect::<String>(),
            sentence
        );
        assert!(tokens.iter().any(|token| token.surface == "見"
            && token.lemma == "見る"
            && token.reading == "み"
            && token.part_of_speech.first().map(String::as_str) == Some("verb")));
        assert!(
            tokens
                .iter()
                .any(|token| token.surface == "。" && !token.lookup_candidate)
        );
        Ok(())
    }

    #[test]
    fn exposes_ipadic_readings_as_hiragana_for_furigana() -> Result<(), AppErrorV1> {
        let tokenizer = LinderaTokenizer::embedded_ipadic()?;
        let tokens = tokenizer.tokenize("航空自衛隊のパイロット")?;
        let readings = tokens
            .iter()
            .map(|token| (token.surface.as_str(), token.reading.as_str()))
            .collect::<Vec<_>>();
        assert!(readings.contains(&("航空", "こうくう")));
        assert!(readings.contains(&("自衛隊", "じえいたい")));
        assert!(readings.contains(&("パイロット", "ぱいろっと")));
        assert!(tokens.iter().all(|token| {
            !token
                .reading
                .chars()
                .any(|character| matches!(character, '\u{30a1}'..='\u{30f6}'))
        }));
        Ok(())
    }

    #[test]
    fn kana_normalization_preserves_non_katakana_symbols() {
        assert_eq!(
            katakana_to_hiragana("パーティー・ヴァージョン"),
            "ぱーてぃー・ゔぁーじょん"
        );
        assert_eq!(katakana_to_hiragana("ABC-123"), "ABC-123");
    }

    #[test]
    fn preserves_spaces_newlines_latin_text_and_emoji() -> Result<(), AppErrorV1> {
        let tokenizer = LinderaTokenizer::embedded_ipadic()?;
        let sentence = "GPUを2個使う 🎬\n次";
        let tokens = tokenizer.tokenize(sentence)?;
        assert_eq!(
            tokens
                .iter()
                .map(|token| token.surface.as_str())
                .collect::<String>(),
            sentence
        );
        for token in tokens {
            assert_eq!(
                sentence.get(token.byte_start as usize..token.byte_end as usize),
                Some(token.surface.as_str())
            );
        }
        Ok(())
    }

    #[test]
    fn token_ids_are_deterministic() -> Result<(), AppErrorV1> {
        let tokenizer = LinderaTokenizer::embedded_ipadic()?;
        let first = tokenizer.tokenize("食べられなかった")?;
        let second = tokenizer.tokenize("食べられなかった")?;
        assert_eq!(first, second);
        assert_eq!(tokenizer.cache_len(), 1);
        assert!(first.iter().any(|token| token.lemma == "食べる"));
        Ok(())
    }

    #[test]
    fn pinned_corpus_has_stable_segmentation() -> Result<(), AppErrorV1> {
        let corpus = include_str!("../../testdata/corpus.json");
        assert!(corpus.contains(TOKENIZER_VERSION));
        let tokenizer = LinderaTokenizer::embedded_ipadic()?;
        for (sentence, expected) in [
            (
                "昨日は映画を見ました。",
                &["昨日", "は", "映画", "を", "見", "まし", "た", "。"][..],
            ),
            ("食べられなかった", &["食べ", "られ", "なかっ", "た"]),
            ("きれいな空", &["きれい", "な", "空"]),
            ("GPUを2個使う", &["GPU", "を", "2", "個", "使う"]),
        ] {
            assert_eq!(
                tokenizer
                    .tokenize(sentence)?
                    .iter()
                    .map(|token| token.surface.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
        Ok(())
    }

    #[cfg(feature = "private-corpus-audit")]
    #[test]
    fn supplied_subtitle_corpus_meets_manual_ninety_percent_gate() -> Result<(), AppErrorV1> {
        let subtitle_source = include_str!(
            "../../../../subtitles/Soratobu.Kouhoushitsu.EP01.1080p.NF.WEB-DL.DDP2.0.H.264-MagicStar.Jpn.srt"
        );
        let gold_source = include_str!("../../testdata/subtitle_audit_gold.tsv");
        let cues = parse_audit_srt(subtitle_source);
        let gold = parse_gold_annotations(gold_source)?;
        assert_eq!(cues.len(), 1_217, "the audited subtitle corpus changed");
        assert!(
            gold.len() >= SUBTITLE_AUDIT_MIN_ANNOTATIONS,
            "the manual audit fixture must not shrink below {SUBTITLE_AUDIT_MIN_ANNOTATIONS} annotations"
        );

        let eligible = cues
            .iter()
            .filter(|(_, text)| text.chars().any(is_japanese_audit_character))
            .collect::<Vec<_>>();
        assert_eq!(eligible.len(), 1_207, "the eligible cue population changed");
        let sampled_ids = (0..SUBTITLE_AUDIT_SAMPLE_SIZE)
            .map(|sample_index| {
                let position =
                    sample_index * (eligible.len() - 1) / (SUBTITLE_AUDIT_SAMPLE_SIZE - 1);
                eligible[position].0
            })
            .collect::<BTreeSet<_>>();
        let annotated_ids = gold
            .iter()
            .map(|annotation| annotation.cue_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            annotated_ids, sampled_ids,
            "gold annotations must cover exactly the reproducible systematic sample"
        );

        let tokenizer = LinderaTokenizer::embedded_ipadic()?;
        let cue_map = cues.into_iter().collect::<BTreeMap<_, _>>();
        let mut segmentation = AuditMetric::default();
        let mut lemma = AuditMetric::default();
        let mut part_of_speech = AuditMetric::default();
        let mut reading = AuditMetric::default();
        let mut mismatches = Vec::new();

        for annotation in &gold {
            let text = &cue_map[&annotation.cue_id];
            let tokens = tokenizer.tokenize(text)?;
            let actual = tokens
                .iter()
                .filter(|token| token.surface == annotation.surface)
                .nth(annotation.occurrence - 1);
            segmentation.record(actual.is_some());
            lemma.record(actual.is_some_and(|token| token.lemma == annotation.lemma));
            part_of_speech.record(actual.is_some_and(|token| {
                token.part_of_speech.first().map(String::as_str) == Some(annotation.part_of_speech)
            }));
            reading.record(actual.is_some_and(|token| token.reading == annotation.reading));
            if let Some(token) = actual {
                if token.lemma != annotation.lemma
                    || token.part_of_speech.first().map(String::as_str)
                        != Some(annotation.part_of_speech)
                    || token.reading != annotation.reading
                {
                    mismatches.push(format!(
                        "cue {} `{}` expected {}/{}/{}; got {}/{}/{}",
                        annotation.cue_id,
                        annotation.surface,
                        annotation.lemma,
                        annotation.part_of_speech,
                        annotation.reading,
                        token.lemma,
                        token
                            .part_of_speech
                            .first()
                            .map_or("unknown", String::as_str),
                        token.reading
                    ));
                }
            } else {
                mismatches.push(format!(
                    "cue {} expected segment `{}` occurrence {}",
                    annotation.cue_id, annotation.surface, annotation.occurrence
                ));
            }
        }

        let total_passed =
            segmentation.passed + lemma.passed + part_of_speech.passed + reading.passed;
        let total_claims = segmentation.total + lemma.total + part_of_speech.total + reading.total;
        println!(
            "subtitle tokenizer audit: {} cues, {} annotations, {} claims; segmentation {}/{} ({:.2}%), lemma {}/{} ({:.2}%), POS {}/{} ({:.2}%), reading {}/{} ({:.2}%), overall {}/{} ({:.2}%)",
            sampled_ids.len(),
            gold.len(),
            total_claims,
            segmentation.passed,
            segmentation.total,
            segmentation.percentage(),
            lemma.passed,
            lemma.total,
            lemma.percentage(),
            part_of_speech.passed,
            part_of_speech.total,
            part_of_speech.percentage(),
            reading.passed,
            reading.total,
            reading.percentage(),
            total_passed,
            total_claims,
            total_passed as f64 * 100.0 / total_claims as f64
        );
        for mismatch in &mismatches {
            println!("audit mismatch: {mismatch}");
        }

        for (name, metric) in [
            ("segmentation", &segmentation),
            ("lemma", &lemma),
            ("part of speech", &part_of_speech),
            ("reading", &reading),
        ] {
            assert!(
                metric.meets_threshold(),
                "{name} score {:.2}% is below the 90% acceptance threshold",
                metric.percentage()
            );
        }
        assert!(
            total_passed * 1_000 >= total_claims * SUBTITLE_AUDIT_THRESHOLD_PER_MILLE,
            "overall score is below the 90% acceptance threshold"
        );
        Ok(())
    }

    #[cfg(feature = "private-corpus-audit")]
    fn parse_gold_annotations(source: &str) -> Result<Vec<GoldAnnotation<'_>>, AppErrorV1> {
        let mut annotations = Vec::new();
        for line in source
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        {
            let columns = line.split('\t').collect::<Vec<_>>();
            if columns.len() != 6 {
                return Err(tokenizer_error(format!("invalid gold row: {line}")));
            }
            let cue_id = columns[0]
                .parse()
                .map_err(|error| tokenizer_error(format!("invalid gold cue id: {error}")))?;
            let occurrence = columns[2]
                .parse()
                .map_err(|error| tokenizer_error(format!("invalid gold occurrence: {error}")))?;
            annotations.push(GoldAnnotation {
                cue_id,
                surface: columns[1],
                occurrence,
                lemma: columns[3],
                part_of_speech: columns[4],
                reading: columns[5],
            });
        }
        Ok(annotations)
    }

    #[cfg(feature = "private-corpus-audit")]
    fn parse_audit_srt(source: &str) -> Vec<(u32, String)> {
        source
            .replace("\r\n", "\n")
            .split("\n\n")
            .filter_map(|block| {
                let mut lines = block.lines();
                let cue_id = lines
                    .next()?
                    .trim()
                    .trim_start_matches('\u{feff}')
                    .parse()
                    .ok()?;
                lines.next()?.contains("-->").then_some(())?;
                Some((cue_id, lines.map(str::trim).collect::<Vec<_>>().join("\n")))
            })
            .collect()
    }

    #[cfg(feature = "private-corpus-audit")]
    fn is_japanese_audit_character(character: char) -> bool {
        matches!(character,
            '\u{3040}'..='\u{30ff}' |
            '\u{3400}'..='\u{4dbf}' |
            '\u{4e00}'..='\u{9fff}' |
            '\u{f900}'..='\u{faff}'
        )
    }
}
