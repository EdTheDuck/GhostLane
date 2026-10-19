@echo off
rem Builds, signs and uploads the Windows installer. See packaging\release-windows.ps1.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0packaging\release-windows.ps1"
pause
