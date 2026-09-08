# Lexical metadata performance audit — 2026-09-06

The optional offline learning database contains 893,228 UniDic CSJ 3.1.1 pitch
patterns and 8,113 community JLPT estimates in a 95,924,224-byte indexed SQLite
file. It is opened read-only and enriches at most the first three ranked JMdict
entries using exact spelling-plus-reading matches.

The metadata layer has its own bounded immutable-result cache: at most 1,024
entries and approximately 8 MiB of serialized payload. This prevents repeated
overlapping subtitle windows from re-running SQLite queries while retaining a
hard memory ceiling. Missing or corrupt metadata degrades to ordinary JMdict
definitions rather than failing dictionary lookup.

Release-mode command:

```powershell
cargo run --release -p dictionary --example lookup_probe -- `
  apps/desktop/src-tauri/release-resources/data/jmdict.sqlite 100 `
  apps/desktop/src-tauri/release-resources/data/lexical-metadata.sqlite
```

Measured on the development host with 10 representative tokens and 100 passes:

| Measurement | Result |
|---|---:|
| Lookups | 1,000 |
| Total lookup time | 15.429 ms |
| Mean lookup time | 15.429 us |
| JMdict results | 4,400 |
| Pitch patterns returned | 4,900 |
| Results carrying a JLPT estimate | 2,100 |

The comparable warmed JMdict-only loop measured 10.254 us per lookup. Metadata
therefore keeps repeated lookup far below the existing 50 ms composed
tokenization/dictionary target. The probe is deterministic workload evidence,
not a substitute for the packaged two-hour memory and p95 release gates.
