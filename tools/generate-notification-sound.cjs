const fs = require('node:fs');
const path = require('node:path');

// Original two-note chime, synthesized locally without external recordings.
const sampleRate = 44100;
const duration = 1.25;
const frames = Math.round(sampleRate * duration);
const samples = new Float64Array(frames * 2);
const notes = [
  { start: 0.055, frequency: 783.990872, gain: 1, pan: -0.12 },
  { start: 0.275, frequency: 1046.502261, gain: 0.86, pan: 0.12 },
];

function bell(t, frequency) {
  if (t < 0) return 0;
  const attack = 1 - Math.exp(-t / 0.006);
  const tail = Math.min(1, Math.max(0, (0.92 - t) / 0.18));
  const phase = 2 * Math.PI * frequency * t;
  return attack * tail * (
    Math.sin(phase) * Math.exp(-5.8 * t)
    + 0.18 * Math.sin(phase * 2) * Math.exp(-10.5 * t)
    + 0.035 * Math.sin(phase * 3.003) * Math.exp(-16 * t)
  );
}

for (let frame = 0; frame < frames; frame += 1) {
  const time = frame / sampleRate;
  for (const note of notes) {
    const t = time - note.start;
    const dry = bell(t, note.frequency);
    const echo = 0.11 * bell(t - 0.085, note.frequency)
      + 0.045 * bell(t - 0.157, note.frequency);
    const value = note.gain * (dry + echo);
    samples[frame * 2] += value * Math.sqrt((1 - note.pan) / 2);
    samples[frame * 2 + 1] += value * Math.sqrt((1 + note.pan) / 2);
  }
}

let peak = 0;
for (const value of samples) peak = Math.max(peak, Math.abs(value));
const gain = 0.22 / peak;
const output = Buffer.alloc(44 + samples.length * 2);
output.write('RIFF', 0);
output.writeUInt32LE(output.length - 8, 4);
output.write('WAVEfmt ', 8);
output.writeUInt32LE(16, 16);
output.writeUInt16LE(1, 20);
output.writeUInt16LE(2, 22);
output.writeUInt32LE(sampleRate, 24);
output.writeUInt32LE(sampleRate * 4, 28);
output.writeUInt16LE(4, 32);
output.writeUInt16LE(16, 34);
output.write('data', 36);
output.writeUInt32LE(samples.length * 2, 40);
let squareSum = 0;
for (let i = 0; i < samples.length; i += 1) {
  const sample = samples[i] * gain;
  squareSum += sample * sample;
  output.writeInt16LE(Math.round(sample * 32767), 44 + i * 2);
}
const destination = path.resolve(__dirname, '../crates/peercast-desktop/frontend/assets/sounds/favorite-notification.wav');
fs.mkdirSync(path.dirname(destination), { recursive: true });
fs.writeFileSync(destination, output);
console.log(JSON.stringify({
  path: destination, duration, sampleRate, channels: 2,
  peakDb: 20 * Math.log10(0.22),
  rmsDb: 20 * Math.log10(Math.sqrt(squareSum / samples.length)),
  bytes: output.length,
}));
