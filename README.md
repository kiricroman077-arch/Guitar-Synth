# Guitar Synth

Plug in a guitar, play single notes, and the plugin plays a synth on the note you're playing.
It listens to the pitch, snaps it to the nearest note and resynthesizes it from scratch.
None of the original guitar sound gets through at 100% wet.

**Stage 1 (current):** monophonic tracking, snap to note, basic waveforms. There's no custom interface yet.

## Install

1. Open the **Actions** tab, click the newest green run, and scroll down to **Artifacts**.
2. Download **GuitarSynth-Windows** (or **GuitarSynth-Mac-AppleSilicon**) and unzip it.
3. Copy `Guitar Synth.vst3` to:
   - **Windows:** `C:\Program Files\Common Files\VST3\`
   - **Mac:** `/Library/Audio/Plug-Ins/VST3/`, then in Terminal run
     `xattr -cr "/Library/Audio/Plug-Ins/VST3/Guitar Synth.vst3"`
4. In Ableton, rescan plug-ins. It's under **Plug-Ins → Audio Effects**.

## Use in Ableton

1. Create an **audio track**, set its input to your guitar's input, and set **Monitor** to **In**.
2. Drop **Guitar Synth** on the track.
3. For less delay, go to **Preferences → Audio** and set the **Buffer Size** to 128 samples or lower.
4. Play **single notes** on a **clean** signal. Put distortion and effects *after* the plugin.

## Controls

| Control | What it does |
|---|---|
| Waveform | Saw, Square or Sine |
| Octave | Shift the synth −2 to +2 octaves |
| Snap to Note | On: locks to the nearest note. Off: follows your exact pitch (bends, vibrato) |
| Input Threshold | How loud the guitar must be to trigger the synth. Raise it if noise triggers notes; lower it if quiet notes get missed |
| Synth Level | Volume of the synth |
| Dry/Wet | 0% = only guitar, 100% = only synth |
