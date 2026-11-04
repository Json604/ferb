"""Package boundary for voice. Must not perform actions or choose task behavior at import time.

FERB has no voice (VOICE-409): nothing here or anywhere else speaks a reply aloud. Push-to-talk is FERB.app's own
dictation; hearing's daemon plumbing is in ferb/daemon/areas/voice.py and ferb/sense/transcribe.py."""
