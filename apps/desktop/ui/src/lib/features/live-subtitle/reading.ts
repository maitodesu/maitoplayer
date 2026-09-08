const KATAKANA_START = 0x30a1;
const KATAKANA_END = 0x30f6;
const KANA_OFFSET = 0x60;
const KATAKANA_EXTENSION_READINGS = new Map(
  Array.from('ㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ').map((character, index) => [
    character,
    Array.from('くしすとぬはひふへほむらりるれろ')[index],
  ]),
);

/** Convert a tokenizer reading to hiragana without touching the displayed surface form. */
export function readingToHiragana(reading: string): string {
  return Array.from(reading.normalize('NFKD'), (character) => {
    const extensionReading = KATAKANA_EXTENSION_READINGS.get(character);
    if (extensionReading) return extensionReading;
    const codePoint = character.codePointAt(0);
    if (codePoint === undefined) return character;
    if (codePoint >= KATAKANA_START && codePoint <= KATAKANA_END) {
      return String.fromCodePoint(codePoint - KANA_OFFSET);
    }
    if (codePoint === 0x30fd || codePoint === 0x30fe) {
      return String.fromCodePoint(codePoint - KANA_OFFSET);
    }
    return character;
  })
    .join('')
    .normalize('NFC');
}
