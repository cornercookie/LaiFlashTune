import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const cap = JSON.parse(readFileSync(new URL('../../src-tauri/capabilities/default.json', import.meta.url), 'utf8'));
const shell = readFileSync(new URL('./Shell.tsx', import.meta.url), 'utf8');

const finishesCloseWithDestroy = shell.includes('.destroy()');
assert.equal(finishesCloseWithDestroy, true, 'close handler must force-destroy after Tauri auto-prevents CloseRequested');
assert.ok(
  cap.permissions.includes('core:window:allow-destroy'),
  'allow-close is not enough: destroy() is denied, so the X button and tray quit never close the window',
);

console.log('closeWindow: ok');
