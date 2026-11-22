@echo off
rem Baut FreeSpeech und legt alles Startfertige nach dist\FreeSpeech.
setlocal
cd /d "%~dp0src-tauri"
cargo build --release || exit /b 1
cd /d "%~dp0"
if not exist dist\FreeSpeech mkdir dist\FreeSpeech
copy /y src-tauri\target\release\FreeSpeech.exe dist\FreeSpeech\ >nul
copy /y engine\*.* dist\FreeSpeech\ >nul
if not exist dist\FreeSpeech\licenses mkdir dist\FreeSpeech\licenses
copy /y licenses\*.* dist\FreeSpeech\licenses\ >nul
echo Fertig: dist\FreeSpeech\FreeSpeech.exe
