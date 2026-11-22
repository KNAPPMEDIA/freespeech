# Fremde Bestandteile / Third-party notices

FreeSpeech selbst steht unter der MIT-Lizenz (siehe `LICENSE`). Diese Datei nennt alles, was nicht von
Knapp Media stammt. Die vollständigen Lizenztexte liegen im Ordner `licenses\` und werden mit dem
Installer ausgeliefert.

## Im Repository und im Installer enthalten

| Bestandteil | Dateien | Herkunft | Lizenz |
| --- | --- | --- | --- |
| transcribe.cpp 0.4.2 (Sprach-Engine) | `engine\transcribe.dll` | [handy-computer/transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) | MIT |
| ggml (Rechenkern der Engine) | `engine\ggml*.dll` | Teil von transcribe.cpp | MIT |
| Silero VAD 6.2 (Sprach-Erkennung von Pausen) | `engine\silero-vad-v6.2-F32.gguf` | [snakers4/silero-vad](https://github.com/snakers4/silero-vad) | MIT |
| Rust-Bindings zu transcribe.cpp | `vendor\transcribe-cpp`, `vendor\transcribe-cpp-sys` | transcribe.cpp 0.4.2 | MIT |
| Microsoft Visual C++ Runtime | `engine\msvcp140*.dll`, `engine\vcruntime140*.dll`, `engine\vcomp140.dll` | Microsoft Visual C++ Redistributable | Microsoft-Lizenz für weitergebbare Laufzeitdateien (kein Open Source, Systembibliothek) |

Die Dateien in `engine\` sind fertig gebaute Binärdateien aus der Veröffentlichung von
[Handy](https://github.com/cjpais/Handy) 0.10.1 (MIT), nicht aus diesem Repository gebaut.
`vendor\transcribe-cpp-sys` ist gegenüber dem Original nur um das Link-Attribut `raw-dylib` ergänzt.

Texte: `licenses\transcribe.cpp-LICENSE.txt`, `licenses\transcribe.cpp-THIRD-PARTY-LICENSES.md`,
`vendor\transcribe-cpp\LICENSE`.

## Zur Laufzeit geladen, nicht enthalten

Sprachmodelle lädt FreeSpeech erst auf Wunsch von HuggingFace. Sie sind weder im Repository noch im Installer.

| Modell | Quelle | Lizenz |
| --- | --- | --- |
| Parakeet Primeline (Deutsch) | [handy-computer/parakeet-primeline-gguf](https://huggingface.co/handy-computer/parakeet-primeline-gguf) | CC BY 4.0 |
| Canary 180M Flash (Deutsch, Englisch, Französisch, Spanisch) | [handy-computer/canary-180m-flash-gguf](https://huggingface.co/handy-computer/canary-180m-flash-gguf) | CC BY 4.0 |

## Rust-Bibliotheken

FreeSpeech wird mit Bibliotheken von crates.io gebaut (Tauri, cpal, arboard und weitere). Die vollständige
Liste mit Version, Lizenz und Quelle steht in `licenses\rust-bibliotheken.md`. Alle Lizenzen sind
freizügig (überwiegend MIT oder Apache-2.0, dazu einzelne BSD, Zlib, Unicode-3.0, BSL-1.0 und MPL-2.0).

## Vorbild

Funktionsumfang und Bedienung orientieren sich an [Handy](https://github.com/cjpais/Handy) von CJ Pais (MIT).
Der Programmcode von FreeSpeech ist eigenständig geschrieben.
