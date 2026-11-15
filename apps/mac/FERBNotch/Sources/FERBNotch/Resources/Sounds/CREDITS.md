# Sound credits

FERB's sounds, rendered by `demo/sound-family/render.py` and stored here losslessly as ALAC in `.caf`.

## From Google's Material Design sound resources (CC BY 4.0)

By Google, licensed under Creative Commons Attribution 4.0 International (CC BY 4.0,
https://creativecommons.org/licenses/by/4.0/). The notification sound was taken from the redistribution at
https://github.com/nana-4/materia-sound-theme.

- `ferb-first-launch.caf`: Material's "hero" sound. Changes: leading silence trimmed, leveled to −24 LUFS.
- `ferb-notification-standard.caf`, `ferb-notification-subtle.caf` and the banners' `ferb-notification-banner.caf`
  (kept at `ui/mac/FERBNotch/`, copied into FERB.app's Resources): Material's "notification, high intensity".
  Changes: trimmed, leveled, a small room added; the Subtle version is also low-passed at 3 kHz.

All files were converted from WAV to ALAC (lossless) for FERB.

## FERB's own

Every other file here is FERB's own, synthesised from code in `render.py`.
