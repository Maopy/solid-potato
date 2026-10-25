@echo off
setlocal
chcp 65001 >nul
cd /d "%~dp0"
rem Reuse this project's isolated runtime when launching a local dist build.
if exist "%~dp0..\..\.venv\Scripts\mitmdump.exe" set "PATH=%~dp0..\..\.venv\Scripts;%PATH%"
rem Optional portable dependencies can be placed beside the executable in tools.
set "PATH=%~dp0tools;%PATH%"
if not exist "%~dp0live-archive-core.exe" (
  echo Missing live-archive-core.exe. Build and extract the Windows release first.
  pause
  exit /b 1
)
"%~dp0live-archive-core.exe" moderation serve --config "%~dp0config\unified.yml"
set "result=%ERRORLEVEL%"
if not "%result%"=="0" pause
exit /b %result%
