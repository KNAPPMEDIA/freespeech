# FreeSpeech

Lokales Diktieren für Windows: Tastenkürzel halten, sprechen, loslassen, der Text steht im aktiven Fenster.
Kein Konto, kein Netz, kein Ton verlässt den Rechner.

Nachbau von [Handy](https://github.com/cjpais/Handy) (MIT) mit derselben Engine
[transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) (MIT), eigener Kern in Rust, eigene Oberfläche.

## Starten
`FreeSpeech.bat` doppelklicken (startet `dist\FreeSpeech\FreeSpeech.exe`).
Das Fenster schließen legt die App nur in den Infobereich, beendet wird über das Symbol dort.

## Bedienen
- **Kürzel** (Standard aus Handy übernommen: Strg links + Win links) halten und sprechen, loslassen fügt ein.
- **Esc** bricht eine laufende Aufnahme ab.
- Modus "Umschalten" statt "Halten" in den Einstellungen.
- Probefeld auf der Startseite zum Testen.
- **Live-Vorschau:** Während du sprichst, steht der bisherige Text in der Anzeige am Bildschirmrand und im Hauptfenster.
  Dafür wird der Ton etwa zweimal pro Sekunde neu transkribiert (bei langen Aufnahmen nur die letzten 30 s).
  Eingefügt wird erst der fertige Text nach dem Loslassen. Abschaltbar unter Einstellungen → Verhalten.
- Solange ein Fenster mit Administratorrechten im Vordergrund ist, sieht FreeSpeech das Kürzel nicht
  (Windows gibt Tasten dort nicht an normale Programme weiter).

## Modelle
FreeSpeech findet `.gguf`-Sprachmodelle an zwei Orten:
- `%APPDATA%\FreeSpeech\models`
- HuggingFace-Cache `%USERPROFILE%\.cache\huggingface\hub` (dort liegen die von Handy geladenen Modelle)

Rechengerät: Standard ist der Prozessor. Die Grafikkarte (Vulkan) ist wählbar.

## Daten
`%APPDATA%\FreeSpeech\settings.json` und `history.json`. Aufnahmen werden nicht gespeichert.

## Fehlersuche
`%APPDATA%\FreeSpeech\freespeech.log` hält Start, Modell-Laden, jedes Diktat (Dauer und Tempo, nie den Text),
Fehler, Paniken und das Beenden fest. Steht dort „Hook war von Windows entfernt worden“, hat Windows den
Tastatur-Hook gekappt und FreeSpeech hat ihn neu gesetzt.

Start: Kürzel und Modell sind kurz nach dem Start bereit („Tastatur-Hook aktiv“, „Modell geladen“). Fenster,
Anzeige und Symbol kommen danach („Oberfläche bereit“); unter hoher Rechnerlast kann das deutlich länger dauern.
Diktieren geht in dieser Zeit schon, nur ohne Anzeige.

Prüfen ohne Mikrofon und ohne Tastatur (Umgebungsvariablen, nur für Tests):
- `FREESPEECH_TEST_WAV=<16-Bit-WAV>`: diese Datei ersetzt die Aufnahme, auch für die Live-Vorschau.
- `FREESPEECH_TEST_AUTORUN=<Sekunden>`: nimmt 5 s nach dem Start einmal so lange auf.
- `FREESPEECH_DEBUG=1`: schreibt Zusatztasten (nie Buchstaben) und den Hook-Takt ins Log.

Beim Testen `paste_method` in `settings.json` auf `none` stellen, sonst landet der Text im aktiven Fenster.

## Weitergeben (Installer)
```
installer.bat
```
erzeugt `src-tauri\target\release\bundle\nsis\FreeSpeech_<Version>_x64-setup.exe` (rund 10 MB) und legt eine Kopie
nach `dist\`. Der Installer bringt Programm, Engine-Dateien und Lizenztexte mit, aber kein Sprachmodell:
Beim ersten Start bietet FreeSpeech an, eines von HuggingFace zu laden (Parakeet Primeline 705 MB oder
Canary 180M Flash 208 MB, beide CC BY 4.0). Installiert wird ohne Administratorrechte nach
`%LOCALAPPDATA%\FreeSpeech`.

Der Installer ist nicht signiert: Windows zeigt beim Empfänger „Der Computer wurde durch Windows geschützt“,
weiter geht es über „Weitere Informationen“ → „Trotzdem ausführen“.

Ersten Start prüfen, ohne die laufende App zu stören: `FREESPEECH_DATA=<leerer Ordner>` startet einen Probelauf
mit eigenem Datenordner, ohne Tastenkürzel und ohne Modelle aus dem HuggingFace-Cache.
`FREESPEECH_TEST_DOWNLOAD=canary` lädt dabei gleich ein Modell.

## Bauen
```
build.bat
```
Braucht Rust (liegt unter `L:\Tools\rust`). Der C++-Kern wird nicht gebaut: `engine\` enthält die fertigen
DLLs von transcribe.cpp 0.4.2, `vendor\transcribe-cpp-sys` bindet sie per `raw-dylib` ein.

## Aufbau
| Ordner | Inhalt |
| --- | --- |
| `src-tauri\src\main.rs` | Ablauf: Kürzel → Aufnahme → Transkription → Einfügen, Befehle für die Oberfläche |
| `src-tauri\src\engine.rs` | transcribe.cpp in eigenem Thread (laden, aufwärmen, transkribieren) |
| `src-tauri\src\audio.rs` | Mikrofon über WASAPI, Umrechnung auf 16 kHz mono |
| `src-tauri\src\hotkey.rs` | globaler Tastatur-Hook (auch Kürzel nur aus Zusatztasten) |
| `src-tauri\src\paste.rs` | Einfügen über Zwischenablage oder direktes Tippen, Signaltöne |
| `src-tauri\src\settings.rs` | Einstellungen, Verlauf, Modell-Suche, Autostart |
| `ui\` | Oberfläche (HTML/CSS/JS ohne Build-Schritt) |
| `engine\` | Laufzeit-DLLs (transcribe, ggml) |
| `vendor\` | Rust-Bindings von transcribe.cpp 0.4.2 |
| `tools\bench` | Messprogramm: Ladezeit und Tempo je Modell und Rechengerät |
| `licenses\` | Lizenztexte der mitgelieferten Teile |
