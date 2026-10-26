# Guitar Synth

Plug in a guitar, play single notes, and the plugin plays a synth on the note you're playing.
It listens to the pitch, snaps it to the nearest note and resynthesizes it from scratch.
None of the original guitar sound gets through at 100% wet.

**Current version:** monophonic tracking with noise rejection, 8 oscillator types, sub oscillator, filter, sustain and pick accent. There's no custom interface yet.

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

**Tracking**

| Control | What it does |
|---|---|
| Input Threshold | How loud the guitar must be to count. Raise it if noise triggers notes; lower it if quiet notes get missed |
| Noise Rejection | How strict the plugin is about what counts as a note. Higher = ignores string touches, scratches and muting noise better, but notes start slightly later and very soft notes may be skipped |
| Snap to Note | On: locks to the nearest note. Off: follows your exact pitch (bends, vibrato) |

**Sound**

| Control | What it does |
|---|---|
| Waveform | Saw, Square, Pulse (PWM), Triangle, Sine, Supersaw, FM, Hard Sync |
| Character | Changes per waveform; see below |
| FM Ratio | FM only: the modulator's pitch ratio. 1 = warm, 2 = hollow/clarinet, 3+ = bell-like and metallic |
| Octave | Shift the synth −2 to +2 octaves |
| Sub Osc | Adds a square wave one octave below |
| Filter Cutoff | Brightness. Lower = darker, rounder |
| Filter Resonance | Adds a peak at the cutoff, from smooth to squelchy and whistling |

What **Character** does for each waveform:

| Waveform | Character |
|---|---|
| Saw, Square | Nothing |
| Pulse (PWM) | Pulse width: 0% = square, higher = thinner and nasal |
| Triangle, Sine | Wavefolding: 0% = pure, higher = brighter and more metallic |
| Supersaw | Detune: 0% = single saw, higher = wide, trance-style chorus |
| FM | FM depth: 0% = pure sine, higher = brighter and more aggressive |
| Hard Sync | Sync amount: higher = the classic tearing, screaming sync lead |

**Dynamics**

| Control | What it does |
|---|---|
| Sustain | 0% = fades exactly like the string. 100% = stays at full volume until you stop the note |
| Pick Accent | How much each pick attack punches: a short boost in volume and brightness |
| Release | How long the synth fades after the note ends |

**Output**

| Control | What it does |
|---|---|
| Synth Level | Volume of the synth |
| Dry/Wet | 0% = only guitar, 100% = only synth |
