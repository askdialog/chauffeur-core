// The sound played when a chat finishes or needs the user while the app is not in front
// (Settings › Notifications, `sound` in `[notifications]`). The app's own sounds are made here, a
// few notes through Web Audio: no file, no licence. A file of the user's is served by the daemon.

import { holdAudio } from "./audioHold";

/** The app's own sounds, by name: notes as [frequency in Hz, start in s, length in s]. */
export const BUILT_IN_SOUNDS: Record<string, { label: string; notes: Array<[number, number, number]> }> = {
  chime: { label: "Chime", notes: [[880, 0, 0.5], [1318.5, 0.12, 0.7]] },
  bell: { label: "Bell", notes: [[1046.5, 0, 1.1]] },
  // An ascending station chime (E, G, C): not the SNCF's, which is protected.
  station: { label: "Station", notes: [[659.3, 0, 0.55], [784, 0.22, 0.55], [1046.5, 0.44, 0.9]] },
};

let context: AudioContext | null = null;

/**
 * The page's audio, created at the first click or key press: a web view only lets a page play
 * sound once the user has done something in it, and a sound played later (the app in the
 * background) comes from that same context. Started once, it rests suspended until a sound plays.
 */
export function unlockAudio() {
  const unlock = () => {
    try {
      context ??= new AudioContext();
      const audio = context;
      audio
        .resume()
        .then(() => {
          window.removeEventListener("pointerdown", unlock, { capture: true });
          window.removeEventListener("keydown", unlock, { capture: true });
          holdAudio(audio)();
        })
        // Refused, it is tried again at the next click or key.
        .catch(() => {});
    } catch {
      // No audio on this machine.
    }
  };
  window.addEventListener("pointerdown", unlock, { capture: true });
  window.addEventListener("keydown", unlock, { capture: true });
}

/** Plays the notes and gives how long they sound, in seconds. */
function playNotes(audio: AudioContext, notes: Array<[number, number, number]>): number {
  const now = audio.currentTime + 0.02;
  let end = 0;
  for (const [frequency, start, length] of notes) {
    const oscillator = audio.createOscillator();
    const gain = audio.createGain();
    oscillator.type = "sine";
    oscillator.frequency.value = frequency;
    // A soft attack and a bell-like decay.
    gain.gain.setValueAtTime(0, now + start);
    gain.gain.linearRampToValueAtTime(0.18, now + start + 0.015);
    gain.gain.exponentialRampToValueAtTime(0.0001, now + start + length);
    oscillator.connect(gain).connect(audio.destination);
    oscillator.start(now + start);
    oscillator.stop(now + start + length + 0.05);
    end = Math.max(end, start + length + 0.05);
  }
  return end;
}

/** Decoded user files, by URL path and setting: fetched once per setting. */
const decoded = new Map<string, Promise<AudioBuffer>>();

/**
 * Plays the sound a setting names: nothing, one of the app's own, or the user's file through the
 * daemon (`fileUrl`). Throws when the file cannot be played, for the settings' "Play" button.
 */
export async function playSound(sound: string, fileUrl: () => string | null): Promise<void> {
  const name = sound.trim();
  if (!name || name === "none") return;
  context ??= new AudioContext();
  const audio = context;
  // Held from now, while a file loads too, until the sound ended.
  const release = holdAudio(audio);
  let seconds = 0;
  try {
    if (audio.state === "suspended") await audio.resume();
    seconds = await play(audio, name, fileUrl);
  } finally {
    setTimeout(release, seconds * 1000);
  }
}

/** Plays a sound in a running context and gives how long it sounds, in seconds. */
async function play(audio: AudioContext, name: string, fileUrl: () => string | null): Promise<number> {
  const builtIn = BUILT_IN_SOUNDS[name];
  if (builtIn) return playNotes(audio, builtIn.notes);
  const url = fileUrl();
  if (!url) throw new Error("not connected to Chauffeur");
  let buffer = decoded.get(name);
  if (!buffer) {
    buffer = fetch(url, { cache: "no-store" })
      .then((response) => {
        if (!response.ok) throw new Error("the daemon cannot play this file");
        return response.arrayBuffer();
      })
      .then((bytes) => audio.decodeAudioData(bytes));
    decoded.set(name, buffer);
    buffer.catch(() => decoded.delete(name));
  }
  const source = audio.createBufferSource();
  source.buffer = await buffer;
  source.connect(audio.destination);
  source.start();
  return source.buffer.duration;
}

/** Forgets the decoded file, after the user picked another or replaced it. */
export function forgetSoundFile() {
  decoded.clear();
}
