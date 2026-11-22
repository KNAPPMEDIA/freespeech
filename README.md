# FreeSpeech

Lokales Diktieren für Windows: Tastenkürzel halten, sprechen, loslassen, der Text steht im aktiven Textfeld.
Kein Konto, keine Cloud, kein Ton verlässt den Rechner. Kostenlos und Open Source (MIT).

*Local dictation for Windows: hold a shortcut, speak, release, and the text appears in the active text field.
No account, no cloud, your voice never leaves your PC. Free and open source (MIT). The app's interface is German.*

Herausgeber: [Knapp Media](https://knapp.media). Vorbild für Funktionsumfang und Bedienung ist
[Handy](https://github.com/cjpais/Handy) (MIT), die Sprach-Engine ist
[transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) (MIT). Kern in Rust, Oberfläche mit Tauri.

## Installieren
Den Installer `FreeSpeech_<Version>_x64-setup.exe` von der Seite
[Releases](https://github.com/KNAPPMEDIA/freespeech/releases) laden und starten. Installiert wird ohne
Administratorrechte nach `%LOCALAPPDATA%\FreeSpeech`, entfernen lässt es sich über „Apps“ in den
Windows-Einstellungen.

- **System:** Windows 11, 64 Bit.
- **Sprachmodell:** nicht im Installer. Beim ersten Start bietet FreeSpeech an, eines von HuggingFace zu laden:
  Parakeet Primeline (Deutsch, rund 740 MB) oder Canary 180M Flash (Deutsch, Englisch, Französisch, Spanisch,
  rund 220 MB).
- **Noch nicht signiert:** Windows zeigt beim Start „Der Computer wurde durch Windows geschützt“. Weiter geht es
  über „Weitere Informationen“ → „Trotzdem ausführen“.

## Bedienen
- **Kürzel** (Standard: Strg links + Win links) halten und sprechen, loslassen fügt ein.
- **Esc** bricht eine laufende Aufnahme ab.
- Modus „Umschalten“ statt „Halten“ in den Einstellungen.
- Probefeld auf der Startseite zum Testen.
- **Live-Vorschau:** Während du sprichst, steht der bisherige Text in der Anzeige am Bildschirmrand und im
  Hauptfenster. Dafür wird der Ton etwa zweimal pro Sekunde neu transkribiert (bei langen Aufnahmen nur die
  letzten 30 s). Eingefügt wird erst der fertige Text nach dem Loslassen. Abschaltbar unter
  Einstellungen → Verhalten.
- Das Fenster schließen legt die App nur in den Infobereich, beendet wird über das Symbol dort.
- Solange ein Fenster mit Administratorrechten im Vordergrund ist, sieht FreeSpeech das Kürzel nicht
  (Windows gibt Tasten dort nicht an normale Programme weiter).

## Datenschutz
FreeSpeech überträgt keine Daten an andere Systeme, außer du stößt es selbst an: Das Laden eines Sprachmodells
holt eine Datei von huggingface.co. Erkennung, Verlauf und Einstellungen bleiben auf deinem Rechner.
Aufnahmen werden nicht gespeichert.

## Modelle
FreeSpeech findet `.gguf`-Sprachmodelle an zwei Orten:
- `%APPDATA%\FreeSpeech\models`
- HuggingFace-Cache `%USERPROFILE%\.cache\huggingface\hub`

Rechengerät: Standard ist der Prozessor. Die Grafikkarte (Vulkan) ist wählbar.

## Daten
`%APPDATA%\FreeSpeech\settings.json` und `history.json`.

## Fehlersuche
`%APPDATA%\FreeSpeech\freespeech.log` hält Start, Modell-Laden, jedes Diktat (Dauer und Tempo, nie den Text),
Fehler, Paniken und das Beenden fest. Steht dort „Hook war von Windows entfernt worden“, hat Windows den
Tastatur-Hook gekappt und FreeSpeech hat ihn neu gesetzt.

Kürzel und Modell sind kurz nach dem Start bereit („Tastatur-Hook aktiv“, „Modell geladen“). Fenster, Anzeige
und Symbol kommen danach („Oberfläche bereit“); unter hoher Rechnerlast kann das deutlich länger dauern.
Diktieren geht in dieser Zeit schon, nur ohne Anzeige.

## Bauen
Voraussetzungen: [Rust](https://rustup.rs) (stable, MSVC) und für den Installer [Node.js](https://nodejs.org).

```
build.bat        Programm bauen, Ergebnis in dist\FreeSpeech
installer.bat    Installer bauen (Tauri, NSIS), Ergebnis in dist\
```

Der C++-Kern wird nicht gebaut: `engine\` enthält die fertigen DLLs von transcribe.cpp 0.4.2,
`vendor\transcribe-cpp-sys` bindet sie per `raw-dylib` ein. Der Installer bringt Programm, Engine-Dateien und
Lizenztexte mit.

Automatisch baut `.github\workflows\windows.yml` den Installer, von Hand gestartet oder bei einem
Versions-Tag (`v0.2.0`). Ein macOS-Build ist vorbereitet (`crates\fs-mac`, `build-mac.sh`), aber ungetestet.

### Prüfen ohne Mikrofon und Tastatur
Umgebungsvariablen, nur für Tests:
- `FREESPEECH_DATA=<leerer Ordner>`: Probelauf mit eigenem Datenordner, ohne Tastenkürzel und ohne Modelle aus
  dem HuggingFace-Cache. Die normal laufende App bleibt unberührt.
- `FREESPEECH_TEST_WAV=<16-Bit-WAV>`: diese Datei ersetzt die Aufnahme, auch für Live-Vorschau und Pegelanzeige.
- `FREESPEECH_TEST_AUTORUN=<Sekunden>`: nimmt nach dem Start einmal so lange auf.
- `FREESPEECH_TEST_DELAY=<Millisekunden>`: Wartezeit bis zu dieser Aufnahme (Standard 5000).
- `FREESPEECH_TEST_DOWNLOAD=canary`: lädt gleich nach dem Start ein Modell.
- `FREESPEECH_DEBUG=1`: schreibt Zusatztasten (nie Buchstaben) und den Hook-Takt ins Log.

Beim Testen `paste_method` in `settings.json` auf `none` stellen, sonst landet der Text im aktiven Fenster.

## Aufbau
| Ordner | Inhalt |
| --- | --- |
| `src-tauri\src\main.rs` | Ablauf: Kürzel → Aufnahme → Transkription → Einfügen, Befehle für die Oberfläche |
| `src-tauri\src\engine.rs` | transcribe.cpp in eigenem Thread (laden, aufwärmen, transkribieren) |
| `src-tauri\src\audio.rs` | Mikrofon über WASAPI, Umrechnung auf 16 kHz mono |
| `src-tauri\src\hotkey.rs` | globaler Tastatur-Hook (auch Kürzel nur aus Zusatztasten) |
| `src-tauri\src\paste.rs` | Einfügen über Zwischenablage oder direktes Tippen, Signaltöne |
| `src-tauri\src\settings.rs` | Einstellungen, Verlauf, Modell-Suche, Autostart |
| `src-tauri\src\download.rs` | Sprachmodelle von HuggingFace laden |
| `ui\` | Oberfläche (HTML/CSS/JS ohne Build-Schritt) |
| `engine\` | Laufzeit-DLLs (transcribe, ggml) |
| `vendor\` | Rust-Bindings von transcribe.cpp 0.4.2 |
| `crates\fs-mac` | Mac-Teile (Tastenkürzel, Einfügen, Berechtigungen), ungetestet |
| `tools\bench` | Messprogramm: Ladezeit und Tempo je Modell und Rechengerät |
| `licenses\` | Lizenztexte der mitgelieferten Teile |

## Mitmachen
Fehler und Wünsche bitte als [Issue](https://github.com/KNAPPMEDIA/freespeech/issues). Änderungen als
Pull Request; jede Änderung von außen wird vor dem Einpflegen von Knapp Media geprüft.

## Lizenz
FreeSpeech steht unter der [MIT-Lizenz](LICENSE). Fremde Bestandteile und ihre Lizenzen stehen in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md), die Sprachmodelle stehen unter CC BY 4.0.
