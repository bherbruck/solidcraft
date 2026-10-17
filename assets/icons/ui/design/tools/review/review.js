// No framework, build step, external resources or model integration.
const S = @@STATE@@;
const fin = new Set(S.final);
const notes = {...S.notes};
let base = {final: [...S.final].sort(), notes: {...notes}};
let filter = 'open', tab = '', dirty = false, timer;
let saves = Promise.resolve();
const query = document.getElementById('q');
const message = document.getElementById('msg');
const cards = [...document.querySelectorAll('.card')];
const markers = [...document.querySelectorAll('.gm')];
const staticKey = 'icon-review:' + S.rev;
const prefsKey = 'icon-review-preferences';
const reloadKey = 'icon-review-reload-view';

function state() {
  return {final: [...fin].sort(), notes: Object.fromEntries(
    Object.entries(notes).filter(([, value]) => value.trim()))};
}
function payload(snapshot) { return JSON.stringify({base, ...snapshot, rev: S.rev}); }
function showMessage(text, error = false) {
  message.textContent = text;
  message.classList.toggle('error', error);
}
function run() {
  let finals = 0, noted = 0, visible = 0;
  for (const card of cards) {
    const id = card.dataset.id;
    const accepted = fin.has(id), hasNote = Boolean((notes[id] || '').trim());
    card.classList.toggle('final', accepted);
    card.classList.toggle('hasnote', hasNote && !accepted);
    card.querySelector('.ck').title = accepted ? 'Reopen for review' : 'Accept as final';
    card.querySelector('.ck').setAttribute('aria-label', (accepted ? 'Reopen ' : 'Accept ') + id + (accepted ? '' : ' as final'));
    if (accepted) finals++;
    if (hasNote && !accepted) noted++;
    const matches = (filter === 'all' || (filter === 'open' && !accepted) ||
      (filter === 'notes' && hasNote && !accepted) || (filter === 'final' && accepted)) &&
      (!tab || card.dataset.tab === tab) && card.dataset.k.includes(query.value.toLowerCase());
    card.hidden = !matches;
    if (matches) visible++;
  }
  for (const marker of markers) {
    let next = marker.nextElementSibling, any = false;
    while (next && !next.classList.contains('gm')) {
      if (!next.hidden) any = true;
      next = next.nextElementSibling;
    }
    marker.hidden = !any;
  }
  document.getElementById('cnt').textContent = `${finals} final · ${cards.length - finals} open · ${noted} with notes`;
  document.getElementById('empty').hidden = visible > 0;
}
function scheduleSave() {
  dirty = true;
  clearTimeout(timer);
  timer = setTimeout(flush, 250);
}
async function flush() {
  clearTimeout(timer);
  const snapshot = state();
  saves = saves.catch(() => {}).then(async () => {
    if (!S.serving) {
      localStorage.setItem(staticKey, JSON.stringify(snapshot));
      dirty = JSON.stringify(snapshot) !== JSON.stringify(state());
      showMessage('Static preview: Export downloads the review file. Use --serve to save to the repository.');
      return;
    }
    const response = await fetch('/api/review', {method: 'POST', body: payload(snapshot)});
    const result = await response.json();
    if (!response.ok || !result.ok) throw new Error(result.error || 'Save failed');
    base = JSON.parse(JSON.stringify(snapshot));
    dirty = JSON.stringify(snapshot) !== JSON.stringify(state());
    showMessage('Saved');
  });
  try { await saves; return true; }
  catch (error) { showMessage(error.message, true); return false; }
}
function view() {
  return {filter, tab, q: query.value, y: scrollY};
}
function preferences() {
  return {dark: document.body.classList.contains('dark'), vision: document.body.dataset.vision, size: document.documentElement.style.getPropertyValue('--sz')};
}
function savePreferences() { try { localStorage.setItem(prefsKey, JSON.stringify(preferences())); } catch {} }
function setSize(size) {
  document.documentElement.style.setProperty('--sz', size + 'px');
  document.body.classList.toggle('small', size <= 24);
  document.querySelectorAll('.sz').forEach(button => button.classList.toggle('on', +button.dataset.s === size));
}
function selectFilter(value) {
  filter = value;
  document.querySelectorAll('[data-f]').forEach(button => button.classList.toggle('on', button.dataset.f === value));
}
function selectTab(value) {
  tab = value;
  document.querySelectorAll('.chip').forEach(button => button.classList.toggle('on', (button.dataset.tab || '') === value));
}

// Temporary palette editor: CSS variables recolour every rendered theme, size
// and vision variant without touching SVGs, notes or acceptance hashes.
const paletteDraftKey = 'icon-review-palette-draft';
const paletteStatus = document.getElementById('palette-status');
const paletteApply = document.getElementById('palette-apply');
const paletteReset = document.getElementById('palette-reset');
const palettePaints = Object.fromEntries(S.palette.tokens.filter(token => !token.linked && !token.knockout)
  .map(token => [token.id, {light: token.light, dark: token.dark}]));
const paletteInputs = [];
let paletteDirty = false, paletteSaving = false;
function paletteVariable(id, theme) { return '--icon-palette-' + id.replaceAll('.', '-') + '-' + theme; }
function paletteChanges() {
  return Object.fromEntries(S.palette.tokens.filter(token => !token.linked && !token.knockout &&
    ['light', 'dark'].some(theme => palettePaints[token.id][theme] !== token[theme]))
    .map(token => [token.id, {...palettePaints[token.id]}]));
}
function updatePalette() {
  for (const [id, paints] of Object.entries(palettePaints)) {
    for (const theme of ['light', 'dark']) document.documentElement.style.setProperty(paletteVariable(id, theme), paints[theme]);
  }
  for (const {input, token, theme} of paletteInputs) {
    input.value = palettePaints[token.linked || token.id][theme];
    input.title = `${token.name} · ${theme}: ${input.value}${token.linked ? ' (follows construction stroke)' : ''}`;
  }
  paletteDirty = Object.keys(paletteChanges()).length > 0;
  paletteApply.disabled = !paletteDirty || paletteSaving;
  paletteReset.disabled = !paletteDirty || paletteSaving;
  try {
    if (paletteDirty) sessionStorage.setItem(paletteDraftKey, JSON.stringify({revision: S.palette.revision, changes: paletteChanges()}));
    else sessionStorage.removeItem(paletteDraftKey);
  } catch {}
}
for (const token of S.palette.tokens) {
  const row = document.createElement('div'); row.className = 'palette-token';
  for (const theme of ['light', 'dark']) {
    if (token.knockout) {
      const swatch = document.createElement('span'); swatch.className = 'palette-knockout';
      swatch.title = token.name + ' · ' + theme + ': transparent cutout';
      swatch.setAttribute('aria-label', swatch.title); row.appendChild(swatch); continue;
    }
    const input = document.createElement('input'); input.type = 'color'; input.value = token[theme];
    input.disabled = Boolean(token.linked);
    input.setAttribute('aria-label', token.name + ' ' + theme + ' colour');
    input.oninput = () => {
      palettePaints[token.id][theme] = input.value;
      updatePalette(); paletteStatus.textContent = 'Preview only · Apply saves both themes to palette.toml';
      paletteStatus.classList.remove('error');
    };
    paletteInputs.push({input, token, theme}); row.appendChild(input);
  }
  const label = document.createElement('span'); label.textContent = token.name + (token.linked ? ' ↗' : '');
  row.appendChild(label); document.getElementById('palette-tokens').appendChild(row);
}
document.body.classList.add('palette-preview');
document.getElementById('palette-all').onclick = () => { selectFilter('all'); selectTab(''); query.value = ''; run(); };
paletteReset.onclick = () => {
  for (const token of S.palette.tokens.filter(token => !token.linked && !token.knockout)) palettePaints[token.id] = {light: token.light, dark: token.dark};
  updatePalette(); paletteStatus.textContent = 'Preview reset · saved palette unchanged'; paletteStatus.classList.remove('error');
};
paletteApply.onclick = async () => {
  if (!paletteDirty || paletteSaving) return;
  if (dirty && !await flush()) return;
  if (!S.serving) {
    paletteStatus.textContent = 'Preview only · run the review server to apply colours to palette.toml'; return;
  }
  paletteSaving = true; updatePalette();
  try {
    const response = await fetch('/api/palette', {method: 'POST', body: JSON.stringify({revision: S.palette.revision, changes: paletteChanges()})});
    const result = await response.json();
    if (!response.ok || !result.ok) throw new Error(result.error || 'Palette save failed');
    S.palette = {revision: result.revision, tokens: result.tokens};
    paletteStatus.textContent = 'Saved to palette.toml · rebuild the app to use these colours';
    paletteStatus.classList.remove('error');
  } catch (error) { paletteStatus.textContent = error.message; paletteStatus.classList.add('error'); }
  finally { paletteSaving = false; updatePalette(); }
};
try {
  const draft = JSON.parse(sessionStorage.getItem(paletteDraftKey) || 'null');
  if (draft?.revision === S.palette.revision) {
    for (const [id, paints] of Object.entries(draft.changes)) {
      if (palettePaints[id] && ['light','dark'].every(theme => /^#[0-9a-f]{6}$/i.test(paints[theme] || ''))) palettePaints[id] = {...paints};
    }
    paletteStatus.textContent = 'Unsaved palette preview restored';
  } else if (draft) { paletteStatus.textContent = 'Saved palette changed · older preview was not restored'; }
} catch {}
updatePalette();
for (const card of cards) {
  const id = card.dataset.id;
  const pop = document.createElement('div');
  pop.className = 'pop';
  const textarea = document.createElement('textarea');
  textarea.placeholder = 'What should change in this icon?';
  textarea.setAttribute('aria-label', 'Notes for ' + id);
  textarea.value = notes[id] || '';
  pop.appendChild(textarea); card.appendChild(pop);
  textarea.oninput = () => { notes[id] = textarea.value; run(); scheduleSave(); };
  card.querySelector('.ck').onclick = () => {
    if (card.dataset.missing === 'true') return;
    fin.has(id) ? fin.delete(id) : fin.add(id);
    card.classList.remove('open'); run(); scheduleSave();
  };
  card.querySelector('.nt').onclick = event => {
    const open = !card.classList.contains('open');
    cards.forEach(other => other.classList.remove('open'));
    card.classList.toggle('open', open);
    if (open) textarea.focus();
    event.stopPropagation();
  };
  pop.onclick = event => event.stopPropagation();
}
document.addEventListener('click', () => cards.forEach(card => card.classList.remove('open')));
document.addEventListener('keydown', event => {
  if (event.key === 'Escape') cards.forEach(card => card.classList.remove('open'));
});
document.querySelectorAll('[data-f]').forEach(button => button.onclick = () => { selectFilter(button.dataset.f); run(); });
document.querySelectorAll('.chip').forEach(button => button.onclick = () => { selectTab(button.dataset.tab || ''); run(); });
document.querySelectorAll('.sz').forEach(button => button.onclick = () => { setSize(+button.dataset.s); savePreferences(); });
document.getElementById('dk').onclick = function () {
  document.body.classList.toggle('dark'); this.classList.toggle('on'); savePreferences();
};
document.getElementById('vision').onchange = event => {
  document.body.dataset.vision = event.target.value; savePreferences();
};
query.oninput = run;
document.getElementById('rescan').onclick = async () => {
  if (!dirty || await flush()) location.reload();
};
document.getElementById('exp').onclick = async () => {
  if (dirty && !await flush()) return;
  if (!S.serving) {
    const snapshot = state();
    const accepted = Object.fromEntries(snapshot.final.map(id => [id, S.hashes[id]]));
    const link = document.createElement('a');
    link.href = URL.createObjectURL(new Blob([JSON.stringify({version: 1, final: accepted, notes: snapshot.notes}, null, 1)], {type: 'application/json'}));
    link.download = 'review.json'; link.click(); URL.revokeObjectURL(link.href);
    return;
  }
  try {
    await saves;
    const snapshot = state();
    const response = await fetch('/api/export', {method: 'POST', body: payload(snapshot)});
    const result = await response.json();
    if (!response.ok || !result.ok) throw new Error(result.error || 'Export failed');
    base = JSON.parse(JSON.stringify(snapshot));
    showMessage(result.sent ? `Queued ${result.sent} note${result.sent === 1 ? '' : 's'} in the local review inbox` : 'No open notes to export');
  } catch (error) { showMessage(error.message, true); }
};
try {
  if (!S.serving) {
    const saved = JSON.parse(localStorage.getItem(staticKey) || 'null');
    if (saved) { fin.clear(); saved.final.forEach(id => fin.add(id)); Object.assign(notes, saved.notes); }
  }
  const prefs = JSON.parse(localStorage.getItem(prefsKey) || 'null');
  if (prefs) {
    if (prefs.vision && ['off', 'red-green', 'blue-yellow'].includes(prefs.vision)) { document.body.dataset.vision = prefs.vision; document.getElementById('vision').value = prefs.vision; }
    if (prefs.dark) document.getElementById('dk').click();
    const size = parseInt(prefs.size);
    if ([16,24,48,64].includes(size)) setSize(size);
  }
  // Only an automatic artwork reload restores filters. A normal opening always
  // starts at Open / Everything, so yesterday's filter cannot hide new artwork.
  const restored = JSON.parse(sessionStorage.getItem(reloadKey) || 'null');
  sessionStorage.removeItem(reloadKey);
  if (restored) {
    selectFilter(restored.filter); selectTab(restored.tab); query.value = restored.q;
    requestAnimationFrame(() => scrollTo(0, restored.y));
  }
} catch {}
run();
if (S.serving) setInterval(async () => {
  if (dirty || paletteDirty || paletteSaving || document.activeElement?.tagName === 'TEXTAREA') return;
  try {
    const response = await fetch('/api/review');
    const current = await response.json();
    if (current.rev && current.rev !== S.rev) {
      sessionStorage.setItem(reloadKey, JSON.stringify(view()));
      location.reload(); return;
    }
    if (JSON.stringify([current.final, current.notes]) !== JSON.stringify([[...fin].sort(), notes])) {
      fin.clear(); current.final.forEach(id => fin.add(id));
      Object.keys(notes).forEach(id => delete notes[id]); Object.assign(notes, current.notes);
      base = {final: [...current.final].sort(), notes: {...notes}};
      cards.forEach(card => { card.querySelector('textarea').value = notes[card.dataset.id] || ''; });
      run();
    }
  } catch { showMessage('Review server unavailable; reconnect before saving.', true); }
}, 3000);
