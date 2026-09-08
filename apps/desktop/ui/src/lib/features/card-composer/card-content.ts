import type { DictionaryEntrySummaryV1 } from '../../contracts/generated';
import { readingToHiragana } from '../live-subtitle/reading';

function unique(values: string[]): string[] {
  return [...new Set(values.map((value) => value.trim()).filter(Boolean))];
}

export function preferredReading(
  tokenReading: string,
  entry?: DictionaryEntrySummaryV1 | null,
): string {
  const normalizedTokenReading = readingToHiragana(tokenReading.trim());
  const dictionaryReadings = unique((entry?.readings ?? []).map(readingToHiragana));
  return (
    dictionaryReadings.find((candidate) => candidate === normalizedTokenReading) ??
    dictionaryReadings[0] ??
    normalizedTokenReading
  );
}

export function formatDictionaryDefinition(entry?: DictionaryEntrySummaryV1 | null): string {
  if (!entry) return '';

  return entry.senses
    .map((sense, index) => {
      const labels = unique([
        ...sense.parts_of_speech,
        ...sense.fields,
        ...sense.dialects,
        ...sense.misc,
      ]);
      const labelText = labels.length > 0 ? ` (${labels.join(', ')})` : '';
      const glosses = unique(sense.glosses).join('; ');
      return glosses ? `${index + 1}.${labelText} ${glosses}` : '';
    })
    .filter(Boolean)
    .join('\n');
}
