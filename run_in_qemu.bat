@echo off
setlocal
cd /d "%~dp0"
cargo +nightly -Zscript scripts\launch.rs %*
exit /b %ERRORLEVEL%
