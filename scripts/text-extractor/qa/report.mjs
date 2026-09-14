import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const escape = value => String(value ?? '').replace(/[&<>"']/gu, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char]));
const visible = value => escape(value).replaceAll(' ', '<span class="space">·</span>');

export async function writeReport(directory, report) {
  const counts = { passed: 0, failed: 0, blocked: 0, pending: 0 };
  for (const result of report.results) counts[result.status]++;
  report.counts = counts;
  // Interrupted or partially executed matrices must never claim a passing run.
  report.status = report.error || report.cleanupErrors.length || counts.blocked || counts.pending ? 'incomplete' : counts.failed ? 'failed' : 'passed';
  await writeFile(join(directory, 'report.json'), JSON.stringify(report, null, 2) + '\n');
  const summary = `${counts.passed} passed · ${counts.failed} failed · ${counts.blocked} blocked · ${counts.pending} not run`;
  const rows = report.results.map(result => {
    const c = result.comparison;
    return `<section class="${escape(result.status)}"><h2>${escape(result.key)} — ${escape(result.status)}</h2>
      <p>${escape(result.error || '')}</p>
      ${c ? `<p>Character error: ${(100 * c.characterErrorRate).toFixed(2)}% · Word error: ${(100 * c.wordErrorRate).toFixed(2)}% · Spaces: ${c.actualSpaces}/${c.expectedSpaces} · Lines: ${c.actualLines}/${c.expectedLines}${c.lostSpacesOnly ? ' · MISSING SPACES REGRESSION' : ''}</p>` : ''}
      <ul>${Object.entries(result.checks || {}).map(([name, passed]) => `<li>${passed ? 'PASS' : 'FAIL'}: ${escape(name)}</li>`).join('')}</ul>
      <div class="texts"><div><h3>Expected</h3><pre>${visible(result.expected)}</pre></div><div><h3>Actual OCR</h3><pre>${visible(result.actual ?? '(not captured)')}</pre></div></div>
      ${result.source ? `<a href="${escape(result.source)}"><img src="${escape(result.source)}" alt="Selected test card"></a>` : ''}
      ${result.editor ? `<p><a href="${escape(result.editor)}">Editor result image</a></p>` : ''}</section>`;
  }).join('\n');
  await writeFile(join(directory, 'report.html'), `<!doctype html><html lang="en"><meta charset="utf-8"><title>Pulse OCR report ${escape(report.id)}</title>
    <style>body{max-width:1250px;margin:40px auto;padding:0 24px;font:15px system-ui;background:#f6f6f6;color:#171717}section{padding:24px;margin:24px 0;background:white;border:1px solid #bbb;border-left:6px solid #888;border-radius:18px}.passed{border-left-color:#18713c}.failed,.blocked{border-left-color:#b32929}.texts{display:grid;grid-template-columns:1fr 1fr;gap:24px}pre{white-space:pre-wrap;overflow-wrap:anywhere;line-height:1.65;background:#f6f6f6;padding:16px}.space{color:#9b9b9b}img{max-width:100%;max-height:380px;object-fit:contain}h2{font-size:19px}h3{font-size:14px}</style>
    <h1>Pulse Basic OCR — ${escape(report.status)}</h1><p>${escape(summary)}</p><p>Run ${escape(report.id)} · revision ${escape(report.revision)} · ${escape(report.platform)}</p>
    <p>${escape(report.error || '')}</p><p>${escape(report.cleanupErrors.join('; '))}</p>
    <p>Dots mark spaces. Internal spacing, punctuation, case, and reading order are compared strictly. This run covers synthetic cards; it does not certify arbitrary applications, HDR, or all monitor configurations.</p>${rows}</html>`);
  await writeFile(join(directory, 'report.md'), `# Pulse Basic OCR: ${report.status}\n\n${summary}\n\nRevision: ${report.revision}\n\n${report.error || ''}\n\n${report.cleanupErrors.join('\n')}\n\n| Case | Status | Character error | Word error |\n| --- | --- | --- | --- |\n${report.results.map(r => `| ${r.key} | ${r.status} | ${r.comparison ? (r.comparison.characterErrorRate * 100).toFixed(2) + '%' : '—'} | ${r.comparison ? (r.comparison.wordErrorRate * 100).toFixed(2) + '%' : '—'} |`).join('\n')}\n\nOpen report.html for source images, expected text, actual text, and interaction checks.\n`);
}
