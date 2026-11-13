# Third-party notices

The window-routing fields and SkyLight ABI in `ferb/act/mac/window_input.py` are adapted from
[Cua](https://github.com/trycua/cua/tree/4cac25bc4a02ff2a52f35c9690d4d9601ccea11c/libs/cua-driver/rust),
`src/input/mouse.rs` and `src/input/skylight.rs`. FERB does not use its activation or focus-transfer recipes.

MIT License

Copyright (c) 2025 Cua AI, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.


The o200k_base token pattern in `ferb/core/context.py` is from OpenAI tiktoken
(https://github.com/openai/tiktoken). Token ranks are loaded from its verified local cache.

MIT License

Copyright (c) 2022 OpenAI, Shantanu Jain

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

### Local memory embedding model

potion-base-8M — https://huggingface.co/minishlab/potion-base-8M (revision bf8b056651a2c21b8d2565580b8569da283cab23)
By MinishLab, MIT License (as its model card states). A static embedding table distilled by
model2vec (https://github.com/MinishLab/model2vec, MIT) from BAAI/bge-base-en-v1.5 (MIT), with its WordPiece
tokenizer. Its three data files (config.json, model.safetensors, tokenizer.json) ship unmodified inside FERB.app
(Contents/Resources/models); a checkout's setup fetches the same files. FERB checks their pinned SHA-256s and reads
them with its own code (ferb/memory/embeddings.py); no model2vec or tokenizer code is copied or installed.

MIT License

Copyright (c) MinishLab (the authors of potion-base-8M and model2vec)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

Sparkle — https://github.com/sparkle-project/Sparkle (version from ui/mac/FERBNotch/Package.swift)
Embedded in FERB.app as Sparkle.framework for automatic updates (L34 Phase G).

Copyright (c) 2006-2013 Andy Matuschak.
Copyright (c) 2009-2013 Elgato Systems GmbH.
Copyright (c) 2011-2014 Kornel Lesiński.
Copyright (c) 2015-2017 Mayur Pawashe.
Copyright (c) 2014 C.W. Betts.
Copyright (c) 2014 Petroules Corporation.
Copyright (c) 2014 Big Nerd Ranch.
All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

=================
EXTERNAL LICENSES
=================

bspatch.c and bsdiff.c, from bsdiff 4.3 <http://www.daemonology.net/bsdiff/>:

Copyright 2003-2005 Colin Percival
All rights reserved

Redistribution and use in source and binary forms, with or without
modification, are permitted providing that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS OR
IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.

--

sais.c and sais.h, from sais-lite (2010/08/07) <https://sites.google.com/site/yuta256/sais>:

The sais-lite copyright is as follows:

Copyright (c) 2008-2010 Yuta Mori All Rights Reserved.

Permission is hereby granted, free of charge, to any person
obtaining a copy of this software and associated documentation
files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use,
copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES
OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

--

Portable C implementation of Ed25519, from https://github.com/orlp/ed25519

Copyright (c) 2015 Orson Peters <orsonpeters@gmail.com>

This software is provided 'as-is', without any express or implied warranty. In no event will the
authors be held liable for any damages arising from the use of this software.

Permission is granted to anyone to use this software for any purpose, including commercial
applications, and to alter it and redistribute it freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not claim that you wrote the
   original software. If you use this software in a product, an acknowledgment in the product
   documentation would be appreciated but is not required.

2. Altered source versions must be plainly marked as such, and must not be misrepresented as
   being the original software.

3. This notice may not be removed or altered from any source distribution.

--

SUSignatureVerifier.m:

Copyright (c) 2011 Mark Hamlin.

All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted providing that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS OR
IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.


The typed cards in the notch (`ui/mac/FERBNotch/Sources/FERBNotch/ResultCards.swift`: reminder, event, note,
expense, contact, link and file cards) rebuild the look of [Shapeshift](https://shapeshiftui.vercel.app)'s intent
cards (`src/components/intents/*.tsx`) in SwiftUI. No Shapeshift code is used, and its text classifier is not:
FERB's own records decide each card.

MIT License

Copyright (c) 2026 Shapeshift contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.


### zstandard (reading compressed Codex sessions)

Memory import reads Codex CLI sessions Codex compressed (`rollout-*.jsonl.zst`, `ferb/memory/import_sources.py`) with
[python-zstandard](https://github.com/indygreg/python-zstandard) 0.25.0, installed unmodified as a dependency. Its
wheels include the [Zstandard](https://github.com/facebook/zstd) library (Meta Platforms, Inc., BSD license).

Copyright (c) 2016, Gregory Szorc
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
this list of conditions and the following disclaimer in the documentation
and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors
may be used to endorse or promote products derived from this software without
specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.


### ds_store and mac_alias (the disk image's window, build time only)

`scripts/build-dmg.sh` writes the disk image's Finder window (`.DS_Store`, with an alias and a bookmark to its
background) with [ds_store](https://github.com/dmgbuild/ds_store) 1.3.3 and
[mac_alias](https://github.com/dmgbuild/mac_alias) 2.2.3 under the MIT License, installed unmodified in the
development environment (uv dev group). Neither ships in FERB.app or the disk image.

Copyright (c) 2014 Alastair Houghton (ds_store, mac_alias)
Copyright (c) 2022 Russell Keith-Magee (mac_alias)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.


### Google Material Design sounds (FERB's first-launch and notification sounds)

Two of FERB.app's sounds come from Google's Material Design sound resources, by Google, licensed under Creative Commons
Attribution 4.0 International (CC BY 4.0, https://creativecommons.org/licenses/by/4.0/); the notification sound was
taken from the redistribution at https://github.com/nana-4/materia-sound-theme. The list of files is in
`ui/mac/FERBNotch/Sources/FERBNotch/Resources/Sounds/CREDITS.md`, which ships beside them in the app.

- First-launch sound (`ferb-first-launch.caf`): Material's "hero" sound. Changes: leading silence trimmed, leveled to
  −24 LUFS, stored as ALAC.
- Notification sound (`ferb-notification-standard.caf`, `ferb-notification-subtle.caf`, `ferb-notification-banner.caf`):
  Material's "notification, high intensity". Changes: trimmed, leveled, a small room added (the Subtle version also
  low-passed at 3 kHz), stored as ALAC.

Every other sound is FERB's own, synthesised from code.

<!-- rust-crates: generated by scripts/rust_notices.py from the Cargo.lock files; do not edit by hand -->

## Rust crates in FERB's Rust build (ferbd, ferb, ferb-native-host)

FERB.app's Rust build compiles these crates into its programs. Each line: the crate and version, its licence expression, and the licence FERB takes it under; then its copyright lines. MIT's and Apache-2.0's texts follow once; the other licences' texts are quoted from the crates' own files. `libsqlite3-sys` compiles in SQLite, which is in the public domain.

- **adler2 2.0.1**: 0BSD OR MIT OR Apache-2.0; used under MIT
  - Copyright (C) Jonas Schievink
- **aead 0.5.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2019 The RustCrypto Project Developers
  - Copyright (c) 2019 MobileCoin, LLC
- **aes 0.8.4**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018 Artyom Pavlov
- **aes-gcm 0.10.3**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2019 The RustCrypto Project Developers
- **aho-corasick 1.1.5**: Unlicense OR MIT; used under MIT
  - Copyright (c) 2015 Andrew Gallant
- **anyhow 1.0.104**: MIT OR Apache-2.0; used under MIT
  - Copyright the anyhow authors (David Tolnay)
- **atomic-waker 1.1.2**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2016 Alex Crichton
  - Copyright (c) 2017 The Tokio Authors
- **base64 0.22.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2015 Alice Maz
- **base64 0.23.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2025 Alice Maz, Marshall Pierce
- **bit-set 0.8.0**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2023 The Rust Project Developers
- **bit-vec 0.8.0**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2023 The Rust Project Developers
- **bitflags 2.13.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 The Rust Project Developers
- **block-buffer 0.10.4**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018-2019 The RustCrypto Project Developers
- **block2 0.6.2**: MIT; used under MIT
  - Copyright the block2 authors (Mads Marquart)
- **bstr 1.13.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018-2019 Andrew Gallant
- **bumpalo 3.20.3**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2019 Nick Fitzgerald
- **bytes 1.12.1**: MIT; used under MIT
  - Copyright (c) 2018 Carl Lerche
- **cfg-if 1.0.5**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 Alex Crichton
- **chacha20 0.10.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2019-2026 The RustCrypto Project Developers
- **chrono 0.4.45**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014, Kang Seonghoon.
- **chrono-tz 0.10.4**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016-2024 Benjamin Sago & the chronotope maintainers
  - Copyright 2016 Djzin
- **cipher 0.4.4**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016-2020 RustCrypto Developers
- **core-foundation-sys 0.8.7**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2012-2013 Mozilla Foundation
- **cpufeatures 0.2.17**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2020-2025 The RustCrypto Project Developers
- **cpufeatures 0.3.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2020-2026 The RustCrypto Project Developers
- **crc32fast 1.5.5**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018 Sam Rijs, Alex Crichton and contributors
- **crypto-common 0.1.7**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2021 RustCrypto Developers
- **ctr 0.9.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018-2022 RustCrypto Developers
  - Copyright (c) 2018 Artyom Pavlov
- **deranged 0.5.9**: MIT OR Apache-2.0; used under MIT
  - Copyright 2024 Jacob Pratt et al.
  - Copyright (c) 2024 Jacob Pratt et al.
- **digest 0.10.7**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2017 Artyom Pavlov
- **dispatch2 0.3.1**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the dispatch2 authors (Mads Marquart, Mary)
- **displaydoc 0.2.7**: MIT OR Apache-2.0; used under MIT
  - Copyright the displaydoc authors (Jane Lusby)
- **equivalent 1.0.3**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2016--2023
- **errno 0.3.15**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 Chris Wong
- **fallible-iterator 0.3.0**: MIT/Apache-2.0; used under MIT
  - Copyright (c) 2015 The rust-openssl-verify Developers
- **fallible-streaming-iterator 0.1.9**: MIT/Apache-2.0; used under MIT
  - Copyright (c) 2016 The fallible-streaming-iterator Developers
- **fancy-regex 0.14.0**: MIT; used under MIT
  - Copyright 2015 The Fancy Regex Authors.
- **fancy-regex 0.19.2**: MIT; used under MIT
  - Copyright 2015 The Fancy Regex Authors.
- **fastrand 2.5.0**: Apache-2.0 OR MIT; used under MIT
  - Copyright the fastrand authors (Stjepan Glavina)
- **flate2 1.1.11**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014-2026 Alex Crichton
- **foldhash 0.2.0**: Zlib; used under Zlib
  - Copyright (c) 2024 Orson Peters
- **form_urlencoded 1.2.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2013-2016 The rust-url developers
- **futures-channel 0.3.34**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016 Alex Crichton
  - Copyright (c) 2017 The Tokio Authors
- **futures-core 0.3.34**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016 Alex Crichton
  - Copyright (c) 2017 The Tokio Authors
- **futures-io 0.3.34**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016 Alex Crichton
  - Copyright (c) 2017 The Tokio Authors
- **futures-macro 0.3.34**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016 Alex Crichton
  - Copyright (c) 2017 The Tokio Authors
- **futures-sink 0.3.34**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016 Alex Crichton
  - Copyright (c) 2017 The Tokio Authors
- **futures-task 0.3.34**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016 Alex Crichton
  - Copyright (c) 2017 The Tokio Authors
- **futures-util 0.3.34**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016 Alex Crichton
  - Copyright (c) 2017 The Tokio Authors
- **generic-array 0.14.7**: MIT; used under MIT
  - Copyright (c) 2015 Bartłomiej Kamiński
- **getrandom 0.2.17**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018-2024 The rust-random Project Developers
  - Copyright (c) 2014 The Rust Project Developers
- **getrandom 0.3.4**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018-2025 The rust-random Project Developers
  - Copyright (c) 2014 The Rust Project Developers
- **getrandom 0.4.3**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018-2026 The rust-random Project Developers
  - Copyright (c) 2014 The Rust Project Developers
- **ghash 0.5.1**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2019 RustCrypto Developers
- **hashbrown 0.17.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2016 Amanieu d'Antras
- **hashlink 0.12.2**: MIT OR Apache-2.0; used under MIT
  - Copyright the hashlink authors
- **hex 0.4.3**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2013-2014 The Rust Project Developers.
  - Copyright (c) 2015-2020 The rust-hex Developers
- **hkdf 0.12.4**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2015-2018 Vlad Filippov
  - Copyright (c) 2018-2021 RustCrypto Developers
- **hmac 0.12.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2017 Artyom Pavlov
- **http 1.5.0**: MIT OR Apache-2.0; used under MIT
  - Copyright 2017 http-rs authors
  - Copyright (c) 2017 http-rs authors
- **http-body 1.1.0**: MIT; used under MIT
  - Copyright (c) 2019-2026 Sean McArthur & Hyper Contributors
- **http-body-util 0.1.5**: MIT; used under MIT
  - Copyright (c) 2019-2026 Sean McArthur & Hyper Contributors
- **httparse 1.10.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2015-2025 Sean McArthur
- **hyper 1.12.0**: MIT; used under MIT
  - Copyright (c) 2014-2026 Sean McArthur
- **hyper-rustls 0.27.10**: Apache-2.0 OR ISC OR MIT; used under MIT
  - Copyright (c) 2016, Joseph Birr-Pixton
  - Copyright (c) 2016 Joseph Birr-Pixton
- **hyper-util 0.1.21**: MIT; used under MIT
  - Copyright (c) 2023-2025 Sean McArthur
- **iana-time-zone 0.1.65**: MIT OR Apache-2.0; used under MIT
  - Copyright 2020 Andrew Straw
  - Copyright (c) 2020 Andrew D. Straw
- **icu_collections 2.4.0**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **icu_locale_core 2.4.0**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **icu_normalizer 2.4.0**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **icu_normalizer_data 2.4.0**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **icu_properties 2.4.0**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **icu_properties_data 2.4.0**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **icu_provider 2.4.0**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **idna 1.1.0**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2013-2025 The rust-url developers
- **idna_adapter 1.2.2**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) The rust-url developers
- **indexmap 2.14.2**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2016--2017
- **inout 0.1.4**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2022 The RustCrypto Project Developers
  - Copyright (c) 2022 Artyom Pavlov
- **ipnet 2.12.2**: MIT OR Apache-2.0; used under MIT
  - Copyright 2017 Juniper Networks, Inc.
- **itoa 1.0.19**: MIT OR Apache-2.0; used under MIT
  - Copyright the itoa authors (David Tolnay)
- **libc 0.2.191**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) The Rust Project Developers
- **libsqlite3-sys 0.40.0**: MIT; used under MIT
  - Copyright (c) 2014 The rusqlite developers
- **litemap 0.8.3**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **log 0.4.35**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 The Rust Project Developers
- **lru-slab 0.1.3**: MIT OR Apache-2.0 OR Zlib; used under MIT
  - Copyright (c) 2024 The lru-slab Developers
- **memchr 2.8.3**: Unlicense OR MIT; used under MIT
  - Copyright (c) 2015 Andrew Gallant
- **miniz_oxide 0.9.1**: MIT OR Zlib OR Apache-2.0; used under MIT
  - Copyright 2013-2014 RAD Game Tools and Valve Software
  - Copyright 2010-2014 Rich Geldreich and Tenacious Software LLC
  - Copyright (c) 2017 Frommi
  - Copyright (c) 2017-2024 oyvindln
  - Copyright (c) 2020 Frommi
- **mio 1.2.4**: MIT; used under MIT
  - Copyright (c) 2014 Carl Lerche and other MIO contributors
- **num-conv 0.2.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) Jacob Pratt
- **num-traits 0.2.19**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 The Rust Project Developers
- **objc2 0.6.5**: MIT; used under MIT
  - Copyright the objc2 authors (Mads Marquart)
- **objc2-app-kit 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-app-kit authors
- **objc2-application-services 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-application-services authors
- **objc2-av-foundation 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-av-foundation authors
- **objc2-cloud-kit 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-cloud-kit authors
- **objc2-core-audio 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-audio authors
- **objc2-core-audio-types 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-audio-types authors
- **objc2-core-data 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-data authors
- **objc2-core-foundation 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-foundation authors
- **objc2-core-graphics 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-graphics authors
- **objc2-core-image 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-image authors
- **objc2-core-media 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-media authors
- **objc2-core-ml 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-ml authors
- **objc2-core-services 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-services authors
- **objc2-core-text 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-text authors
- **objc2-core-video 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-core-video authors
- **objc2-encode 4.1.0**: MIT; used under MIT
  - Copyright the objc2-encode authors (Mads Marquart)
- **objc2-foundation 0.3.2**: MIT; used under MIT
  - Copyright the objc2-foundation authors
- **objc2-image-io 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-image-io authors
- **objc2-io-surface 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-io-surface authors
- **objc2-metal 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-metal authors
- **objc2-quartz-core 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-quartz-core authors
- **objc2-screen-capture-kit 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-screen-capture-kit authors
- **objc2-security 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-security authors
- **objc2-uniform-type-identifiers 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-uniform-type-identifiers authors
- **objc2-vision 0.3.2**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright the objc2-vision authors
- **once_cell 1.21.4**: MIT OR Apache-2.0; used under MIT
  - Copyright the once_cell authors (Aleksey Kladov)
- **opaque-debug 0.3.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018-2024 The RustCrypto Project Developers
- **percent-encoding 2.3.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2013-2025 The rust-url developers
- **phf 0.12.1**: MIT; used under MIT
  - Copyright (c) 2014-2022 Steven Fackler, Yuki Okushi
- **phf_shared 0.12.1**: MIT; used under MIT
  - Copyright (c) 2014-2022 Steven Fackler, Yuki Okushi
- **pin-project-lite 0.2.17**: Apache-2.0 OR MIT; used under MIT
  - Copyright the pin-project-lite authors
- **plist 1.10.3**: MIT; used under MIT
  - Copyright (c) 2015 Edward Barnard
- **polyval 0.6.2**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2019-2023 RustCrypto Developers
- **potential_utf 0.1.6**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **powerfmt 0.2.1**: MIT OR Apache-2.0; used under MIT
  - Copyright 2023 Jacob Pratt et al.
  - Copyright (c) 2023 Jacob Pratt et al.
- **proc-macro2 1.0.107**: MIT OR Apache-2.0; used under MIT
  - Copyright the proc-macro2 authors (David Tolnay, Alex Crichton)
- **quick-xml 0.42.0**: MIT; used under MIT
  - Copyright (c) 2016 Johann Tuffe
- **quinn 0.11.12**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018 The quinn Developers
- **quinn-proto 0.11.19**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018 The quinn Developers
- **quinn-udp 0.5.16**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018 The quinn Developers
- **quote 1.0.47**: MIT OR Apache-2.0; used under MIT
  - Copyright the quote authors (David Tolnay)
- **rand 0.10.3**: MIT OR Apache-2.0; used under MIT
  - Copyright 2018 Developers of the Rand project
  - Copyright (c) 2014 The Rust Project Developers
- **rand_core 0.10.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018-2026 The Rand Project Developers
- **rand_core 0.6.4**: MIT OR Apache-2.0; used under MIT
  - Copyright 2018 Developers of the Rand project
  - Copyright (c) 2014 The Rust Project Developers
- **rand_pcg 0.10.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014-2017 Melissa O'Neill and PCG Project contributors
  - Copyright 2018 Developers of the Rand project
- **regex 1.13.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 The Rust Project Developers
- **regex-automata 0.4.18**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 The Rust Project Developers
- **regex-syntax 0.8.11**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 The Rust Project Developers
- **reqwest 0.12.28**: MIT OR Apache-2.0; used under MIT
  - Copyright 2016 Sean McArthur
  - Copyright (c) 2016-2025 Sean McArthur
- **ring 0.17.14**: Apache-2.0 AND ISC; used under Apache-2.0 AND ISC
  - Copyright (c) 2009 The Go Authors. All rights reserved.
  - Copyright 2015 The Chromium Authors. All rights reserved.
  - Copyright 2015-2025 Brian Smith.
- **roxmltree 0.20.0**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018 Yevhenii Reizner
- **rusqlite 0.42.0**: MIT; used under MIT
  - Copyright (c) 2014 The rusqlite developers
- **rustc-hash 2.1.3**: Apache-2.0 OR MIT; used under MIT
  - Copyright the rustc-hash authors (The Rust Project Developers)
- **rustix 1.1.6**: Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT; used under MIT
  - Copyright the rustix authors (Dan Gohman, Jakub Konka)
- **rustls 0.23.46**: Apache-2.0 OR ISC OR MIT; used under MIT
  - Copyright (c) 2016, Joseph Birr-Pixton
  - Copyright (c) 2016 Joseph Birr-Pixton
- **rustls-pki-types 1.16.0**: MIT OR Apache-2.0; used under MIT
  - Copyright 2023 Dirkjan Ochtman
  - Copyright (c) 2023 Dirkjan Ochtman
- **rustls-webpki 0.103.17**: ISC; used under ISC
  - Copyright 2015 Brian Smith.
- **ryu 1.0.23**: Apache-2.0 OR BSL-1.0; used under Apache-2.0
  - Copyright the ryu authors (David Tolnay)
- **serde 1.0.229**: MIT OR Apache-2.0; used under MIT
  - Copyright the serde authors (Erick Tryzelaar, David Tolnay)
- **serde_core 1.0.229**: MIT OR Apache-2.0; used under MIT
  - Copyright the serde_core authors (Erick Tryzelaar, David Tolnay)
- **serde_derive 1.0.229**: MIT OR Apache-2.0; used under MIT
  - Copyright the serde_derive authors (Erick Tryzelaar, David Tolnay)
- **serde_json 1.0.154**: MIT OR Apache-2.0; used under MIT
  - Copyright the serde_json authors
- **serde_urlencoded 0.7.1**: MIT/Apache-2.0; used under MIT
  - Copyright (c) 2016 Anthony Ramine
- **sha1_smol 1.0.1**: BSD-3-Clause; used under BSD-3-Clause
  - Copyright (c) 2018, the respective contributors, as shown by the AUTHORS file.
  - Copyright (c) 2006-2009 Graydon Hoare
  - Copyright (c) 2009-2013 Mozilla Foundation
- **sha2 0.10.9**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2006-2009 Graydon Hoare
  - Copyright (c) 2009-2013 Mozilla Foundation
  - Copyright (c) 2016 Artyom Pavlov
- **signal-hook-registry 1.4.8**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2017 tokio-jsonrpc developers
- **simd-adler32 0.3.10**: MIT; used under MIT
  - Copyright (c) [2021] [Marvin Countryman]
- **siphasher 1.0.4**: MIT OR Apache-2.0; used under MIT
  - Copyright 2012-2016 The Rust Project Developers.
  - Copyright 2016-2026 Frank Denis.
- **slab 0.4.12**: MIT; used under MIT
  - Copyright (c) 2019 Carl Lerche
- **smallvec 1.16.5**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2018 The Servo Project Developers
- **socket2 0.6.5**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2014 Alex Crichton
- **stable_deref_trait 1.2.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2017 Robert Grosse
- **subtle 2.6.1**: BSD-3-Clause; used under BSD-3-Clause
  - Copyright (c) 2016-2017 Isis Agora Lovecruft, Henry de Valence. All rights reserved.
  - Copyright (c) 2016-2024 Isis Agora Lovecruft. All rights reserved.
- **syn 3.0.10**: MIT OR Apache-2.0; used under MIT
  - Copyright the syn authors (David Tolnay)
- **sync_wrapper 1.0.2**: Apache-2.0; used under Apache-2.0
  - Copyright the sync_wrapper authors (Actyx AG)
- **synstructure 0.14.0**: MIT; used under MIT
  - Copyright 2016 Nika Layzell
- **tempfile 3.27.0**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2015 Steven Allen
- **thiserror 2.0.21**: MIT OR Apache-2.0; used under MIT
  - Copyright the thiserror authors (David Tolnay)
- **thiserror-impl 2.0.21**: MIT OR Apache-2.0; used under MIT
  - Copyright the thiserror-impl authors (David Tolnay)
- **tiktoken-rs 0.12.1**: MIT; used under MIT
  - Copyright the tiktoken-rs authors (Roger Zurawicki)
- **time 0.3.55**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) Jacob Pratt et al.
- **time-core 0.1.9**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) Jacob Pratt et al.
- **time-macros 0.2.32**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) Jacob Pratt et al.
- **tinystr 0.8.5**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **tinyvec 1.13.3**: Zlib OR Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2019 Daniel "Lokathor" Gee.
- **tokio 1.54.1**: MIT; used under MIT
  - Copyright (c) Tokio Contributors
- **tokio-macros 2.7.2**: MIT; used under MIT
  - Copyright (c) 2019 Yoshua Wuyts
  - Copyright (c) Tokio Contributors
- **tokio-rustls 0.26.6**: MIT OR Apache-2.0; used under MIT
  - Copyright 2017 quininer kel
  - Copyright (c) 2017 quininer kel
- **tokio-util 0.7.20**: MIT; used under MIT
  - Copyright (c) Tokio Contributors
- **toml_datetime 0.6.11**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) Individual contributors
- **toml_edit 0.22.27**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) Individual contributors
- **toml_write 0.1.2**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) Individual contributors
- **tower 0.5.3**: MIT; used under MIT
  - Copyright (c) 2019 Tower Contributors
- **tower-http 0.6.11**: MIT; used under MIT
  - Copyright (c) 2019-2021 Tower Contributors
- **tower-layer 0.3.3**: MIT; used under MIT
  - Copyright (c) 2019 Tower Contributors
- **tower-service 0.3.3**: MIT; used under MIT
  - Copyright (c) 2019 Tower Contributors
- **tracing 0.1.44**: MIT; used under MIT
  - Copyright (c) 2019 Tokio Contributors
- **tracing-core 0.1.36**: MIT; used under MIT
  - Copyright (c) 2019 Tokio Contributors
- **try-lock 0.2.5**: MIT; used under MIT
  - Copyright (c) 2018-2023 Sean McArthur
  - Copyright (c) 2016 Alex Crichton
- **typenum 1.20.1**: MIT OR Apache-2.0; used under MIT
  - Copyright 2014 Paho Lurie-Gregg
  - Copyright (c) 2014 Paho Lurie-Gregg
- **unicode-ident 1.0.27**: (MIT OR Apache-2.0) AND Unicode-3.0; used under MIT AND Unicode-3.0
  - Copyright © 1991-2023 Unicode, Inc.
- **unicode-normalization 0.1.26**: (MIT OR Apache-2.0) AND Unicode-3.0; used under MIT AND Unicode-3.0
  - Copyright (c) 2015 The Rust Project Developers
  - Copyright © 1991-2023 Unicode, Inc.
- **universal-hash 0.5.1**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2019-2020 RustCrypto Developers
- **untrusted 0.9.0**: ISC; used under ISC
  - Copyright the untrusted authors (Brian Smith)
- **url 2.5.8**: MIT OR Apache-2.0; used under MIT
  - Copyright (c) 2013-2025 The rust-url developers
- **utf8_iter 1.1.0**: Apache-2.0 OR MIT; used under MIT
  - Copyright the utf8_iter authors (Henri Sivonen)
- **utf_types 1.0.0**: Apache-2.0 OR MIT; used under MIT
  - Copyright the utf_types authors (Henri Sivonen)
- **uuid 1.29.2**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2014 The Rust Project Developers
  - Copyright (c) 2018 Ashley Mannix, Christopher Armstrong, Dylan DPC, Hunar Roop Kahlon
- **want 0.3.2**: MIT; used under MIT
  - Copyright (c) 2018-2019 Sean McArthur
- **webpki-roots 1.0.12**: CDLA-Permissive-2.0; used under CDLA-Permissive-2.0
  - Copyright the webpki-roots authors
- **winnow 0.7.15**: MIT; used under MIT
  - Copyright the winnow authors
- **writeable 0.6.4**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **yoke 0.8.4**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **yoke-derive 0.8.4**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **zerofrom 0.1.8**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **zerofrom-derive 0.1.8**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **zeroize 1.9.1**: Apache-2.0 OR MIT; used under MIT
  - Copyright (c) 2018-2026 The RustCrypto Project Developers
- **zerotrie 0.2.5**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **zerovec 0.11.9**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **zerovec-derive 0.11.7**: Unicode-3.0; used under Unicode-3.0
  - Copyright © 2020-2024 Unicode, Inc.
- **zip 2.6.1**: MIT; used under MIT
  - Copyright (c) 2014 Mathijs van de Nes
- **zmij 1.0.23**: MIT; used under MIT
  - Copyright the zmij authors (David Tolnay)
- **zopfli 0.8.4**: Apache-2.0; used under Apache-2.0
  - Copyright 2011 Google Inc.

### MIT

```
Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

### Apache-2.0

```
Apache License
                        Version 2.0, January 2004
                     https://www.apache.org/licenses/LICENSE-2.0

TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

1. Definitions.

   "License" shall mean the terms and conditions for use, reproduction,
   and distribution as defined by Sections 1 through 9 of this document.

   "Licensor" shall mean the copyright owner or entity authorized by
   the copyright owner that is granting the License.

   "Legal Entity" shall mean the union of the acting entity and all
   other entities that control, are controlled by, or are under common
   control with that entity. For the purposes of this definition,
   "control" means (i) the power, direct or indirect, to cause the
   direction or management of such entity, whether by contract or
   otherwise, or (ii) ownership of fifty percent (50%) or more of the
   outstanding shares, or (iii) beneficial ownership of such entity.

   "You" (or "Your") shall mean an individual or Legal Entity
   exercising permissions granted by this License.

   "Source" form shall mean the preferred form for making modifications,
   including but not limited to software source code, documentation
   source, and configuration files.

   "Object" form shall mean any form resulting from mechanical
   transformation or translation of a Source form, including but
   not limited to compiled object code, generated documentation,
   and conversions to other media types.

   "Work" shall mean the work of authorship, whether in Source or
   Object form, made available under the License, as indicated by a
   copyright notice that is included in or attached to the work
   (an example is provided in the Appendix below).

   "Derivative Works" shall mean any work, whether in Source or Object
   form, that is based on (or derived from) the Work and for which the
   editorial revisions, annotations, elaborations, or other modifications
   represent, as a whole, an original work of authorship. For the purposes
   of this License, Derivative Works shall not include works that remain
   separable from, or merely link (or bind by name) to the interfaces of,
   the Work and Derivative Works thereof.

   "Contribution" shall mean any work of authorship, including
   the original version of the Work and any modifications or additions
   to that Work or Derivative Works thereof, that is intentionally
   submitted to Licensor for inclusion in the Work by the copyright owner
   or by an individual or Legal Entity authorized to submit on behalf of
   the copyright owner. For the purposes of this definition, "submitted"
   means any form of electronic, verbal, or written communication sent
   to the Licensor or its representatives, including but not limited to
   communication on electronic mailing lists, source code control systems,
   and issue tracking systems that are managed by, or on behalf of, the
   Licensor for the purpose of discussing and improving the Work, but
   excluding communication that is conspicuously marked or otherwise
   designated in writing by the copyright owner as "Not a Contribution."

   "Contributor" shall mean Licensor and any individual or Legal Entity
   on behalf of whom a Contribution has been received by Licensor and
   subsequently incorporated within the Work.

2. Grant of Copyright License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   copyright license to reproduce, prepare Derivative Works of,
   publicly display, publicly perform, sublicense, and distribute the
   Work and such Derivative Works in Source or Object form.

3. Grant of Patent License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   (except as stated in this section) patent license to make, have made,
   use, offer to sell, sell, import, and otherwise transfer the Work,
   where such license applies only to those patent claims licensable
   by such Contributor that are necessarily infringed by their
   Contribution(s) alone or by combination of their Contribution(s)
   with the Work to which such Contribution(s) was submitted. If You
   institute patent litigation against any entity (including a
   cross-claim or counterclaim in a lawsuit) alleging that the Work
   or a Contribution incorporated within the Work constitutes direct
   or contributory patent infringement, then any patent licenses
   granted to You under this License for that Work shall terminate
   as of the date such litigation is filed.

4. Redistribution. You may reproduce and distribute copies of the
   Work or Derivative Works thereof in any medium, with or without
   modifications, and in Source or Object form, provided that You
   meet the following conditions:

   (a) You must give any other recipients of the Work or
       Derivative Works a copy of this License; and

   (b) You must cause any modified files to carry prominent notices
       stating that You changed the files; and

   (c) You must retain, in the Source form of any Derivative Works
       that You distribute, all copyright, patent, trademark, and
       attribution notices from the Source form of the Work,
       excluding those notices that do not pertain to any part of
       the Derivative Works; and

   (d) If the Work includes a "NOTICE" text file as part of its
       distribution, then any Derivative Works that You distribute must
       include a readable copy of the attribution notices contained
       within such NOTICE file, excluding those notices that do not
       pertain to any part of the Derivative Works, in at least one
       of the following places: within a NOTICE text file distributed
       as part of the Derivative Works; within the Source form or
       documentation, if provided along with the Derivative Works; or,
       within a display generated by the Derivative Works, if and
       wherever such third-party notices normally appear. The contents
       of the NOTICE file are for informational purposes only and
       do not modify the License. You may add Your own attribution
       notices within Derivative Works that You distribute, alongside
       or as an addendum to the NOTICE text from the Work, provided
       that such additional attribution notices cannot be construed
       as modifying the License.

   You may add Your own copyright statement to Your modifications and
   may provide additional or different license terms and conditions
   for use, reproduction, or distribution of Your modifications, or
   for any such Derivative Works as a whole, provided Your use,
   reproduction, and distribution of the Work otherwise complies with
   the conditions stated in this License.

5. Submission of Contributions. Unless You explicitly state otherwise,
   any Contribution intentionally submitted for inclusion in the Work
   by You to the Licensor shall be under the terms and conditions of
   this License, without any additional terms or conditions.
   Notwithstanding the above, nothing herein shall supersede or modify
   the terms of any separate license agreement you may have executed
   with Licensor regarding such Contributions.

6. Trademarks. This License does not grant permission to use the trade
   names, trademarks, service marks, or product names of the Licensor,
   except as required for reasonable and customary use in describing the
   origin of the Work and reproducing the content of the NOTICE file.

7. Disclaimer of Warranty. Unless required by applicable law or
   agreed to in writing, Licensor provides the Work (and each
   Contributor provides its Contributions) on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
   implied, including, without limitation, any warranties or conditions
   of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
   PARTICULAR PURPOSE. You are solely responsible for determining the
   appropriateness of using or redistributing the Work and assume any
   risks associated with Your exercise of permissions under this License.

8. Limitation of Liability. In no event and under no legal theory,
   whether in tort (including negligence), contract, or otherwise,
   unless required by applicable law (such as deliberate and grossly
   negligent acts) or agreed to in writing, shall any Contributor be
   liable to You for damages, including any direct, indirect, special,
   incidental, or consequential damages of any character arising as a
   result of this License or out of the use or inability to use the
   Work (including but not limited to damages for loss of goodwill,
   work stoppage, computer failure or malfunction, or any and all
   other commercial damages or losses), even if such Contributor
   has been advised of the possibility of such damages.

9. Accepting Warranty or Additional Liability. While redistributing
   the Work or Derivative Works thereof, You may choose to offer,
   and charge a fee for, acceptance of support, warranty, indemnity,
   or other liability obligations and/or rights consistent with this
   License. However, in accepting such obligations, You may act only
   on Your own behalf and on Your sole responsibility, not on behalf
   of any other Contributor, and only if You agree to indemnify,
   defend, and hold each Contributor harmless for any liability
   incurred by, or claims asserted against, such Contributor by reason
   of your accepting any such warranty or additional liability.

END OF TERMS AND CONDITIONS
```

### BSD-3-Clause (sha1_smol 1.0.1)

```
BSD 3-Clause License

Copyright (c) 2018, the respective contributors, as shown by the AUTHORS file.
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the copyright holder nor the names of its
  contributors may be used to endorse or promote products derived from
  this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

-----

src/simd.rs is licensed under the MIT license:

Copyright (c) 2006-2009 Graydon Hoare
Copyright (c) 2009-2013 Mozilla Foundation

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

### BSD-3-Clause (subtle 2.6.1)

```
Copyright (c) 2016-2017 Isis Agora Lovecruft, Henry de Valence. All rights reserved.
Copyright (c) 2016-2024 Isis Agora Lovecruft. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### CDLA-Permissive-2.0 (webpki-roots 1.0.12)

```
# Community Data License Agreement - Permissive - Version 2.0

This is the Community Data License Agreement - Permissive, Version
2.0 (the "agreement"). Data Provider(s) and Data Recipient(s) agree
as follows:

## 1. Provision of the Data

1.1. A Data Recipient may use, modify, and share the Data made
available by Data Provider(s) under this agreement if that Data
Recipient follows the terms of this agreement.

1.2. This agreement does not impose any restriction on a Data
Recipient's use, modification, or sharing of any portions of the
Data that are in the public domain or that may be used, modified,
or shared under any other legal exception or limitation.

## 2. Conditions for Sharing Data

2.1. A Data Recipient may share Data, with or without modifications, so
long as the Data Recipient makes available the text of this agreement
with the shared Data.

## 3. No Restrictions on Results

3.1. This agreement does not impose any restriction or obligations
with respect to the use, modification, or sharing of Results.

## 4. No Warranty; Limitation of Liability

4.1. All Data Recipients receive the Data subject to the following
terms:

THE DATA IS PROVIDED ON AN "AS IS" BASIS, WITHOUT REPRESENTATIONS,
WARRANTIES OR CONDITIONS OF ANY KIND, EITHER EXPRESS OR IMPLIED
INCLUDING, WITHOUT LIMITATION, ANY WARRANTIES OR CONDITIONS OF TITLE,
NON-INFRINGEMENT, MERCHANTABILITY OR FITNESS FOR A PARTICULAR PURPOSE.

NO DATA PROVIDER SHALL HAVE ANY LIABILITY FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING
WITHOUT LIMITATION LOST PROFITS), HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE DATA OR RESULTS,
EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGES.

## 5. Definitions

5.1. "Data" means the material received by a Data Recipient under
this agreement.

5.2. "Data Provider" means any person who is the source of Data
provided under this agreement and in reliance on a Data Recipient's
agreement to its terms.

5.3. "Data Recipient" means any person who receives Data directly
or indirectly from a Data Provider and agrees to the terms of this
agreement.

5.4. "Results" means any outcome obtained by computational analysis
of Data, including for example machine learning models and models'
insights.
```

### ISC (ring 0.17.14)

```
*ring* uses an "ISC" license, like BoringSSL used to use, for new code
files. See LICENSE-other-bits for the text of that license.

See LICENSE-BoringSSL for code that was sourced from BoringSSL under the
Apache 2.0 license. Some code that was sourced from BoringSSL under the ISC
license. In each case, the license info is at the top of the file.

See src/polyfill/once_cell/LICENSE-APACHE and src/polyfill/once_cell/LICENSE-MIT
for the license to code that was sourced from the once_cell project.
```

### ISC (ring 0.17.14)

```
Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.


Licenses for support code
-------------------------

Parts of the TLS test suite are under the Go license. This code is not included
in BoringSSL (i.e. libcrypto and libssl) when compiled, however, so
distributing code linked against BoringSSL does not trigger this license:

Copyright (c) 2009 The Go Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google Inc. nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.


BoringSSL uses the Chromium test infrastructure to run a continuous build,
trybots etc. The scripts which manage this, and the script for generating build
metadata, are under the Chromium license. Distributing code linked against
BoringSSL does not trigger this license.

Copyright 2015 The Chromium Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google Inc. nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### ISC (ring 0.17.14)

```
Copyright 2015-2025 Brian Smith.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### ISC (rustls-webpki 0.103.17)

```
Except as otherwise noted, this project is licensed under the following
(ISC-style) terms:

Copyright 2015 Brian Smith.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

The files under third-party/chromium are licensed as described in
third-party/chromium/LICENSE.
```

### ISC (untrusted 0.9.0)

```
// Copyright 2015-2016 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### Unicode-3.0 (icu_collections 2.4.0, icu_locale_core 2.4.0, icu_normalizer 2.4.0, icu_normalizer_data 2.4.0, icu_properties 2.4.0, icu_properties_data 2.4.0, icu_provider 2.4.0, litemap 0.8.3, potential_utf 0.1.6, tinystr 0.8.5, writeable 0.6.4, yoke 0.8.4, yoke-derive 0.8.4, zerofrom 0.1.8, zerofrom-derive 0.1.8, zerotrie 0.2.5, zerovec 0.11.9, zerovec-derive 0.11.7)

```
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 2020-2024 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.

SPDX-License-Identifier: Unicode-3.0

—

Portions of ICU4X may have been adapted from ICU4C and/or ICU4J.
ICU 1.8.1 to ICU 57.1 © 1995-2016 International Business Machines Corporation and others.
```

### Unicode-3.0 (unicode-ident 1.0.27, unicode-normalization 0.1.26)

```
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 1991-2023 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.
```

### Zlib (foldhash 0.2.0)

```
Copyright (c) 2024 Orson Peters

This software is provided 'as-is', without any express or implied warranty. In
no event will the authors be held liable for any damages arising from the use of
this software.

Permission is granted to anyone to use this software for any purpose, including
commercial applications, and to alter it and redistribute it freely, subject to
the following restrictions:

1. The origin of this software must not be misrepresented; you must not claim
    that you wrote the original software. If you use this software in a product,
    an acknowledgment in the product documentation would be appreciated but is
    not required.

2. Altered source versions must be plainly marked as such, and must not be
    misrepresented as being the original software.

3. This notice may not be removed or altered from any source distribution.
```

<!-- /rust-crates -->
