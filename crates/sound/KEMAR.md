# The ear responses in `src/kemar.bin`

`src/kemar.bin` holds the MIT Media Lab's diffuse-field equalized KEMAR head-related impulse
responses (the "diffuse" set: 128 taps at 44.1 kHz, 368 directions from 40° below to straight
above), packed into one file for the sound mix (`src/mix.rs`).

> HRTF Measurements of a KEMAR Dummy-Head Microphone.
> Bill Gardner and Keith Martin, MIT Media Lab, 1994.
> <https://sound.media.mit.edu/resources/KEMAR.html>

Copyright 1994 by the MIT Media Laboratory. The data is provided free with no restrictions on use,
provided the authors are cited.

## The file

All little-endian: `HRIR`, the sample rate (u32), the taps per response (u16) and the number of
elevation rings (u16); then each ring, lowest first: its elevation in degrees (i16) and how many
directions it has (u16); then each direction, azimuth ascending from 0 (ahead) to 180 (behind)
on the right: its azimuth in degrees (u16), then its responses as 16-bit samples, the left ear's
and the right's interleaved. Directions on the left are the right's mirrored, as the set's own
documentation has it.
