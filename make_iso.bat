@echo off
setlocal
cd /d "%~dp0"
cargo +nightly -Zscript scripts\make_iso.rs %*
exit /b %ERRORLEVEL%
