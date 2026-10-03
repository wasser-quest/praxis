'use strict';
const toggle = document.getElementById('menu-toggle');
const nav = document.getElementById('navigation');
function closeMenu() {
  toggle.setAttribute('aria-expanded', 'false');
  nav.classList.remove('is-open');
}
toggle.addEventListener('click', () => {
  const open = toggle.getAttribute('aria-expanded') !== 'true';
  toggle.setAttribute('aria-expanded', String(open));
  nav.classList.toggle('is-open', open);
});
nav.addEventListener('click', event => { if (event.target.closest('a')) closeMenu(); });
document.addEventListener('keydown', event => {
  if (event.key === 'Escape' && toggle.getAttribute('aria-expanded') === 'true') {
    closeMenu();
    toggle.focus();
  }
});
window.matchMedia('(min-width: 761px)').addEventListener('change', event => { if (event.matches) closeMenu(); });
async function copyCommands(text) {
  if (navigator.clipboard && window.isSecureContext) {
    try { await navigator.clipboard.writeText(text); return true; } catch (_) { /* try selection fallback */ }
  }
  const previous = document.activeElement;
  const selection = window.getSelection();
  const ranges = Array.from({ length: selection.rangeCount }, (_, i) => selection.getRangeAt(i).cloneRange());
  const field = document.createElement('textarea');
  field.value = text;
  field.setAttribute('readonly', '');
  field.style.cssText = 'position:fixed;left:-9999px;top:0;';
  document.body.appendChild(field);
  try { field.select(); return document.execCommand('copy'); }
  finally {
    field.remove();
    selection.removeAllRanges();
    ranges.forEach(range => selection.addRange(range));
    if (previous) previous.focus({ preventScroll: true });
  }
}
document.querySelectorAll('[data-copy-target]').forEach(button => {
  button.addEventListener('click', async () => {
    const content = document.getElementById(button.dataset.copyTarget);
    if (!content) return;
    button.disabled = true;
    let copied = false;
    try { copied = await copyCommands(content.textContent); } catch (_) { /* report failure */ }
    button.textContent = copied ? 'Copied' : 'Select & copy';
    document.getElementById('copy-status').textContent = copied ? 'Setup commands copied.' : 'Copy unavailable. Select the commands and copy them manually.';
    button.disabled = false;
    window.setTimeout(() => { button.textContent = 'Copy commands'; }, 2400);
  });
});
