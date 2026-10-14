// The soft blip for a terminal bell. Generated with WebAudio, so there is no sound file to ship.
// Browsers only let a page make sound after the person has touched it, so the audio context is
// created on the first tap or key press; a bell before that is silent.

import { getBellSound } from './prefs';

let ctx: AudioContext | null = null;

function unlock() {
  try {
    ctx ??= new AudioContext();
    void ctx.resume();
  } catch {
    /* no WebAudio here */
  }
}

/** Call once at start-up. */
export function armBell() {
  for (const type of ['pointerdown', 'keydown'] as const) {
    document.addEventListener(type, unlock, { once: true, capture: true });
  }
}

export function playBell() {
  if (!getBellSound() || !ctx || ctx.state !== 'running') return;
  const t = ctx.currentTime;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = 'sine';
  osc.frequency.setValueAtTime(880, t);
  osc.frequency.exponentialRampToValueAtTime(660, t + 0.12);
  gain.gain.setValueAtTime(0.0001, t);
  gain.gain.exponentialRampToValueAtTime(0.06, t + 0.01);
  gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.16);
  osc.connect(gain).connect(ctx.destination);
  osc.start(t);
  osc.stop(t + 0.18);
}
