@echo off
setlocal

for %%I in ("%~dp0..") do set "AGENT_ROOT=%%~fI"

set "VCVARS=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if not exist "%VCVARS%" (
    echo Visual Studio C++ Build Tools not found.
    exit /b 1
)
call "%VCVARS%" >nul
if errorlevel 1 exit /b %ERRORLEVEL%

set "SDK_TOOLS=%LOCAL_AGENT_SDK_TOOLS%"
if not defined SDK_TOOLS set "SDK_TOOLS=%USERPROFILE%\Desktop\myProgram\On_top_Clock\.tools"
set "SDK_CORE=%SDK_TOOLS%\sdk-core\c"
set "SDK_X64=%SDK_TOOLS%\sdk-x64\c"

if not exist "%SDK_X64%\um\x64\kernel32.lib" (
    if not exist "%WindowsSdkDir%Lib\%WindowsSDKVersion%um\x64\kernel32.lib" (
        echo Windows SDK libraries not found. Install the Windows SDK or set LOCAL_AGENT_SDK_TOOLS to a folder containing sdk-core and sdk-x64.
        exit /b 1
    )
) else (
    set "PATH=%SDK_CORE%\bin\10.0.26100.0\x64;%PATH%"
    set "LIB=%SDK_X64%\um\x64;%SDK_X64%\ucrt\x64;%LIB%"
    set "INCLUDE=%SDK_CORE%\Include\10.0.26100.0\um;%SDK_CORE%\Include\10.0.26100.0\shared;%SDK_CORE%\Include\10.0.26100.0\ucrt;%SDK_CORE%\Include\10.0.26100.0\winrt;%INCLUDE%"
)

if "%~1"=="--check" goto check
if "%~1"=="--test" goto test

call "%AGENT_ROOT%\node_modules\.bin\tauri.cmd" %*
exit /b %ERRORLEVEL%

:check
cd /d "%AGENT_ROOT%\src-tauri"
cargo check
exit /b %ERRORLEVEL%

:test
cd /d "%AGENT_ROOT%\src-tauri"
cargo test
exit /b %ERRORLEVEL%
