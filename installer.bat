@echo off
rem Baut den Windows-Installer (NSIS) ueber den Tauri-Bundler und legt ihn nach dist\.
setlocal
cd /d "%~dp0"
call npx --yes @tauri-apps/cli@2 build || exit /b 1
if not exist dist mkdir dist
copy /y src-tauri\target\release\bundle\nsis\FreeSpeech_*_x64-setup.exe dist\ >nul
echo Fertig: dist\FreeSpeech_*_x64-setup.exe
