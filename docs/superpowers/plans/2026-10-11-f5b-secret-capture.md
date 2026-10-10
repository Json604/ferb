# F5b: secret capture and task perception

Goal: enforce the decided secret classes before persistence/model input, deliver one working-app picture when the worker starts using it, and observe destructive classification without granting it authority.

Architecture: the shared Redactor owns text/field classes; on-device Vision OCR and AX frames feed a fail-closed JPEG sanitizer used by gateway images and recording frames. Hidden prompts pass values directly to bound fill operations and optionally to Keychain, never model arguments. The gateway attaches the first target-window picture to the first successful app observation; later pictures require explicit screenshot calls. Jev classifies actions in forced shadow. FDA setup declares the full-access prerequisite only after containment is wired.

Spec: D8/D12/D13/D28/P5/P18 secret classes include secure fields/Secure Input, token patterns, contextual OTPs, Luhn-valid payment cards, excluded password managers and concealed/transient clipboard. D6/D26/D29 pictures never substitute desktop/current browser tab; unavailable capture is a recorded fact. CQ2 classifier cannot affect execution. CQ5 setup never grants FDA automatically.

Global constraints: fresh ferb/ implementation; no new limits; no live probes, installs, restarts or external sends. Author regression tests first, compile during implementation; owner-directed final combined tests/review deferred through F10. AGENTS.md owner edits untouched.

Review focus: all image sinks (tool replies, evidence, recordings), secret lifetime and exact target binding, first-look concurrency, OCR failures, admin profile containment and no raw hidden values in exception text.

1. Secret text/field classes and stream regressions. Files: sanitize/redact.py, tests/test_secret_classes.py. Add field(text, role, label, secure_input, concealed); payment checksum and contextual OTP recognition; retain existing token and URL handling. Commit D8/D28/P5.
2. Screenshot sanitizer and first-look routing. Files: sanitize/images.py, daemon/gateway.py, act/mac/tools.py, act/recording.py, tests/test_screenshot_redaction.py. Inject OCR/masking dependencies for offline tests. Fail closed on recognizer failure; mask rectangles before every sink. Commit D13/D6/D26/D29.
3. Generalized hidden fills and explicit Keychain persistence. Files: daemon/questions.py, daemon/gateway.py, cli/chat.py, identity/secrets.py, browser/mac tools. Bind prompt to current element/origin; ensure cancellation and secret leases; test raw value never appears in result/trace. Commit D12/P5.
4. Destructive shadow, labelled set, FDA/docs. Files: decide/questions.py, daemon/main.py, daemon/gateway.py, eval/sets, setup, docs. Forced shadow regardless config; document accuracy unmeasured; setup prerequisites evidence and residuals. Commit D23/CQ2/D22/CQ5.

Self-review: compilation and diff checks during work; no claim of runtime OCR, sudo, FDA, or live security effectiveness until final tests and owner acceptance.
