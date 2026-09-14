// Normalize platform line endings and visual indentation only. Never collapse
// internal spaces, punctuation, case, or line order to make OCR look successful.
export function normalize(text) {
  return String(text).normalize('NFC').replace(/\r\n?/gu, '\n').replace(/\u00a0/gu, ' ')
    .split('\n').map(line => line.trim()).join('\n').trim();
}
function distance(a, b) {
  let previous = Array.from({ length: b.length + 1 }, (_, i) => i);
  for (let i = 0; i < a.length; i++) {
    const row = [i + 1];
    for (let j = 0; j < b.length; j++) row.push(Math.min(row[j] + 1, previous[j + 1] + 1, previous[j] + (a[i] === b[j] ? 0 : 1)));
    previous = row;
  }
  return previous[b.length];
}
export function compare(expectedText, actualText) {
  const expected = normalize(expectedText), actual = normalize(actualText);
  const expectedWords = expected.match(/\S+/gu) || [], actualWords = actual.match(/\S+/gu) || [];
  const spaces = value => (value.match(/ /gu) || []).length;
  const characterEdits = distance([...expected], [...actual]);
  const wordEdits = distance(expectedWords, actualWords);
  return {
    pass: expected === actual, expected, actual, characterEdits, wordEdits,
    characterErrorRate: characterEdits / Math.max(1, [...expected].length),
    wordErrorRate: wordEdits / Math.max(1, expectedWords.length),
    expectedWords: expectedWords.length, actualWords: actualWords.length,
    expectedSpaces: spaces(expected), actualSpaces: spaces(actual),
    lostSpacesOnly: expected !== actual && expected.replaceAll(' ', '') === actual.replaceAll(' ', '') && spaces(actual) < spaces(expected),
    expectedLines: expected ? expected.split('\n').length : 0,
    actualLines: actual ? actual.split('\n').length : 0,
  };
}
