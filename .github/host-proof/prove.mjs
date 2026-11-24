// Fork only, never offered upstream: proves the host extension points in the real wasm build
// before a release is published (host-release.yml). A bare page host, made here from the
// release's own index.html (data-host="page" on the canvas and a log of every gridcraft:* event
// the app sends), drives each slot in headless Chrome as a hosting page would.
//   npm install && node prove.mjs <unzipped gridcraft-web-<version>/> <out dir> [chrome]
import { createServer } from 'node:http';
import { readFile, stat, writeFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import puppeteer from 'puppeteer-core';

const [SITE, OUT, CHROME = '/usr/bin/google-chrome'] = process.argv.slice(2);

// The host page: the release's index.html, hosted in its own document, logging what the app sends.
const LOG = `<script>
window.__log = [];
{
  const c = document.getElementById('gridcraft_canvas');
  for (const k of ['ready', 'save', 'dirty', 'failed', 'open-request', 'document']) c.addEventListener('gridcraft:' + k, (e) => {
    const d = e.detail || {};
    window.__log.push({ kind: k, ...Object.fromEntries(Object.entries(d).filter(([n]) => n !== 'bytes')), bytes: d.bytes ? d.bytes.byteLength : undefined, at: performance.now() });
  });
}
</script>
`;
const CANVAS = '<canvas id="gridcraft_canvas"></canvas>';
const index = await readFile(join(SITE, 'index.html'), 'utf8');
if (!index.includes(CANVAS)) throw new Error('index.html has no plain gridcraft_canvas to host');
await writeFile(join(SITE, 'host.html'), index.replace(CANVAS, '<canvas id="gridcraft_canvas" data-host="page"></canvas>').replace('</body>', LOG + '</body>'));
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css' };
const WORDS = 'Budget from GitHub';
const KEY = 'acme/books@main:Budget.xlsx';
const LABEL = 'acme/books · main · Budget.xlsx';

const server = createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  const file = normalize(join(SITE, path));
  try { await stat(file); res.writeHead(200, { 'Content-Type': TYPES[extname(file)] ?? 'application/octet-stream' }).end(await readFile(file)); }
  catch { res.writeHead(404).end(); }
});
await new Promise((ok) => server.listen(0, '127.0.0.1', ok));
const browser = await puppeteer.launch({ executablePath: CHROME, headless: true,
  args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--no-sandbox'] });

const results = [];
let failed = false;
const check = (name, ok, evidence) => {
  results.push({ name, ok, evidence });
  if (!ok) failed = true;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}\n      ${JSON.stringify(evidence)}`);
};

try {
  const page = await browser.newPage();
  await page.setViewport({ width: 1400, height: 900 });
  // Every time the app asks the browser for a file: a file input clicked or put on the page.
  await page.evaluateOnNewDocument(() => {
    window.__filePickers = 0;
    const click = HTMLInputElement.prototype.click;
    HTMLInputElement.prototype.click = function () {
      if (this.type === 'file') window.__filePickers += 1;
      return click.call(this);
    };
    new MutationObserver((records) => {
      for (const r of records) for (const n of r.addedNodes) if (n instanceof Element && (n.matches('input[type=file]') || n.querySelector('input[type=file]'))) window.__filePickers += 1;
    }).observe(document, { childList: true, subtree: true });
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/host.html`);
  await page.waitForFunction(() => window.__log.some((e) => e.kind === 'ready') && typeof window.gridcraft === 'object', { timeout: 180_000 });

  // Helpers in the page: send a host message, run a command, read the log.
  await page.evaluate(() => {
    const c = document.getElementById('gridcraft_canvas');
    window.send = (k, d) => c.dispatchEvent(new CustomEvent('gridcraft:' + k, { detail: d }));
    window.run = (id, p = {}) => window.gridcraft.execute(id, p).then((v) => ({ ok: v }), (e) => ({ err: String(e) }));
    window.since = (t) => window.__log.filter((e) => e.at > t);
  });
  const now = () => page.evaluate(() => performance.now());
  const run = (id, p) => page.evaluate((i, q) => window.run(i, q), id, p);
  const send = (k, d) => page.evaluate((a, b) => window.send(a, b), k, d);
  const since = (t) => page.evaluate((x) => window.since(x), t);
  const pause = (ms) => new Promise((ok) => setTimeout(ok, ms));
  const a1 = async () => JSON.stringify((await run('cell.get', { cell: 'A1' })).ok ?? '');
  const shows = async (w) => {
    for (let i = 0; i < 60; i++) {
      if ((await a1()).includes(w)) return;
      await pause(1000);
    }
    throw new Error('the workbook never showed ' + w);
  };
  const activeDirty = async () => (await run('window.list')).ok?.find((w) => w.active)?.dirty;

  // The workbook the host keeps: made by the app itself, then put away.
  await run('cell.set', { cell: 'A1', input: WORDS });
  const encoded = (await run('file.saveBytes', { format: 'xlsx' })).ok?.base64;
  if (!encoded) throw new Error('the app could not encode a workbook to host');
  const workbook = Array.from(Buffer.from(encoded, 'base64'));
  await run('file.close', { force: true, document: true });
  await pause(500);
  const hostOpens = async () => {
    await page.evaluate((bytes) => window.send('open', { name: 'Budget.xlsx', bytes: new Uint8Array(bytes).buffer }), workbook);
    await send('document-place', { place: 'github', key: KEY, label: LABEL });
    await shows(WORDS);
    await pause(500);
  };

  // ── Slot 1: host places ──────────────────────────────────────────────────────────────
  const none = await run('ui.hostPlaces');
  check('no places before the host declares any', none.ok?.places?.length === 0, none);
  await send('places', { places: [
    { id: 'github', label: 'GitHub', open: true, saveAs: true },
    { id: 'inbox', label: 'Shared inbox', open: true, saveAs: false },
    { id: 'bad', label: 'Neither' },
  ] });
  await pause(300);
  const listed = (await run('ui.hostPlaces')).ok;
  check('gridcraft:places is listed (the entry offering neither is dropped)', listed?.places?.map((p) => p.id).join() === 'github,inbox', listed);

  await run('ui.dialog', { name: 'open' });
  await pause(800);
  const owned = await page.evaluate(() => ({ filePickers: window.__filePickers }));
  check('a host with open places owns Open: no browser picker is asked for', owned.filePickers === 0, owned);
  await page.screenshot({ path: join(OUT, '1-backstage-open-places.png') });
  await run('ui.backstage', { page: 'close' });

  let t = await now();
  const asked = await run('ui.hostOpen', { place: 'github' });
  await pause(200);
  const requests = (await since(t)).filter((e) => e.kind === 'open-request');
  check('choosing a place sends gridcraft:open-request {place}', asked.ok?.asked === 'github' && requests.length === 1 && requests[0].place === 'github' && requests[0].key === undefined, { asked, requests });

  // The host answers with the workbook and where it lives.
  await hostOpens();
  const where = (await run('ui.hostPlaces')).ok?.document;
  check('gridcraft:document-place is the open workbook\'s place', where?.label === LABEL && where?.key === KEY, where);
  await run('ui.backstage', { page: 'info' });
  await pause(800);
  await page.screenshot({ path: join(OUT, '2-backstage-info-location.png') });
  await run('ui.backstage', { page: 'open' });
  await pause(800);
  await page.screenshot({ path: join(OUT, '3-backstage-open-recent.png') });
  await run('ui.backstage', { page: 'close' });
  await pause(300);

  // ── Save, Save As and exports ────────────────────────────────────────────────────────
  await run('cell.set', { cell: 'B1', input: 'Edited' });
  t = await now();
  let r = await run('file.save');
  await pause(300);
  let saves = (await since(t)).filter((e) => e.kind === 'save');
  check('before confirm-saves a Save carries no id, saveAs false, export false, no auto', r.ok && saves.length === 1 && saves[0].id === undefined && saves[0].saveAs === false && saves[0].export === false && saves[0].auto === undefined && saves[0].name === 'Budget.xlsx', { r, saves });
  t = await now();
  await run('ui.dialog', { name: 'saveAs' });
  await pause(300);
  saves = (await since(t)).filter((e) => e.kind === 'save');
  check('ui.dialog {name: "saveAs"} sends save with saveAs: true', saves.length === 1 && saves[0].saveAs === true && saves[0].export === false, saves);
  t = await now();
  await run('file.exportPdf');
  await pause(500);
  saves = (await since(t)).filter((e) => e.kind === 'save');
  check('an export is sent with export: true, never numbered', saves.length === 1 && saves[0].export === true && saves[0].id === undefined && saves[0].name.endsWith('.pdf'), saves);

  // ── Slot 3: confirmed saves ──────────────────────────────────────────────────────────
  await send('confirm-saves', { value: true });
  await run('cell.set', { cell: 'B2', input: 'Again' });
  t = await now();
  r = await run('file.save');
  await pause(500);
  saves = (await since(t)).filter((e) => e.kind === 'save');
  const id1 = saves[0]?.id;
  check('after confirm-saves a Save carries an id and is pending', typeof id1 === 'string' && r.ok?.pendingSave === id1, { r, saves });
  check('while a save waits for its answer the host never hears the workbook is clean', !(await since(t)).some((e) => e.kind === 'dirty' && e.dirty === false), await since(t));
  check('a save the host has not confirmed still counts as unsaved', (await activeDirty()) === true, await run('window.list'));
  t = await now();
  await send('save-failed', { id: id1, message: 'the branch has moved on' });
  await pause(800);
  await page.screenshot({ path: join(OUT, '4-save-failed-message.png') });
  check('gridcraft:save-failed leaves the workbook unsaved (the host never hears clean)', !(await since(t)).some((e) => e.kind === 'dirty' && e.dirty === false) && (await activeDirty()) === true, await since(t));
  await run('ui.message.dismiss');

  t = await now();
  r = await run('file.save');
  await pause(300);
  const id2 = (await since(t)).find((e) => e.kind === 'save')?.id;
  t = await now();
  await send('saved', { id: id2 });
  await pause(800);
  check('gridcraft:saved {id} confirms it: the host hears the workbook is clean', typeof id2 === 'string' && id2 !== id1 && (await since(t)).some((e) => e.kind === 'dirty' && e.dirty === false) && (await activeDirty()) === false, { id2, log: await since(t) });

  await run('cell.set', { cell: 'B3', input: 'To another place' });
  t = await now();
  r = await run('ui.hostSaveAs', { place: 'github' });
  await pause(300);
  saves = (await since(t)).filter((e) => e.kind === 'save');
  check('a Save As into a place carries place and saveAs', r.ok && saves.length === 1 && saves[0].place === 'github' && saves[0].saveAs === true, { r, saves });
  if (saves[0]?.id) await send('saved', { id: saves[0].id });
  const inbox = await run('ui.hostSaveAs', { place: 'inbox' });
  check('Save As into a place that takes no saves is refused', inbox.err !== undefined, inbox);
  await pause(500);

  // ── Slot 1 again: the Recent entry reopens through the host ──────────────────────────
  // Clicked on the backstage Open page's Recent row, as a person would. The row is found, not
  // assumed: down the Open page's column, the first click that asks the host for a workbook by
  // key is the row (the place buttons above it ask without a key).
  await run('ui.backstage', { page: 'open' });
  await pause(800);
  t = await now();
  let reopen = [];
  for (let y = 100; y <= 760 && reopen.length === 0; y += 12) {
    await page.mouse.click(420, y);
    await pause(250);
    reopen = (await since(t)).filter((e) => e.kind === 'open-request' && e.key !== undefined);
    await run('ui.backstage', { page: 'open' });
  }
  check('a Recent workbook from a place reopens through gridcraft:open-request {place, key}', reopen.length === 1 && reopen[0].key === KEY && reopen[0].place === 'github', reopen);
  await run('ui.backstage', { page: 'close' });
  await hostOpens();
  await page.screenshot({ path: join(OUT, '5-after-recent-click.png') });

  // ── Slot 2: AutoSave ─────────────────────────────────────────────────────────────────
  const refused = await run('file.autosave', { value: true });
  check('without gridcraft:autosave, AutoSave stays off with the browser reason', /browser/i.test(refused.err ?? ''), refused);
  await send('autosave', { available: true });
  await pause(300);
  const on = await run('file.autosave', { value: true });
  check('gridcraft:autosave {available: true} lets AutoSave turn on', on.ok?.value === true, on);
  await page.screenshot({ path: join(OUT, '6-autosave-on.png') });
  // GridCraft's AutoSave can write in the very frame that made the change: watch from before it.
  t = await now();
  await run('cell.set', { cell: 'C1', input: 'Typed with AutoSave on' });
  await pause(6000);
  saves = (await since(t)).filter((e) => e.kind === 'save');
  check('AutoSave writes through the host on its own (no Save pressed)', saves.length >= 1 && saves[0].name === 'Budget.xlsx' && typeof saves[0].id === 'string', saves);
  check('a save AutoSave sent carries auto: true', saves.length >= 1 && saves.every((s) => s.auto === true), saves);
  for (const s of saves) await send('saved', { id: s.id });
  await send('autosave', { available: false });
  await pause(300);
  const off = await run('file.autosave', { value: true });
  check('the host can take AutoSave back', off.err !== undefined, off);
  await run('file.autosave', { value: false });

  // ── The workbook shown ───────────────────────────────────────────────────────────────
  const budgetId = (await page.evaluate(() => window.__log.filter((e) => e.kind === 'document').at(-1)))?.id;
  t = await now();
  await run('file.new');
  await pause(500);
  const made = (await since(t)).filter((e) => e.kind === 'document');
  check('gridcraft:document tells the host of a new workbook (no file name yet)', made.length >= 1 && made.at(-1).open === true && made.at(-1).name === undefined && made.at(-1).id !== budgetId, { made, budgetId });
  t = await now();
  await run('window.activate', { doc: Number(budgetId) });
  await pause(500);
  const back = (await since(t)).filter((e) => e.kind === 'document');
  check('gridcraft:document tells the host when the first is shown again', back.length >= 1 && back.at(-1).id === budgetId && back.at(-1).name === 'Budget.xlsx' && back.at(-1).documents?.includes(made.at(-1)?.id), { back, budgetId });
} finally {
  await writeFile(join(OUT, 'results.json'), JSON.stringify(results, null, 1));
  await browser.close();
  server.close();
}
console.log(failed ? 'SOME CHECKS FAILED' : 'ALL CHECKS PASSED');
process.exit(failed ? 1 : 0);
