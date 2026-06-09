@echo off
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0make_iso.ps1" %*
exit /b %ERRORLEVEL%
