@echo off
chcp 65001 >nul
setlocal

rem ============================================================
rem  Build the signed Android APK (arm64) and copy it to the
rem  repository root as RunHelp-v<version>-android-arm64-signed.apk
rem
rem  Designed to run non-interactively (e.g. from RunHelp pipeline):
rem  no pause, no prompt, non-zero exit code on failure.
rem ============================================================

cd /d "%~dp0.."

set "JAVA_HOME=D:\Android\Studio\android-studio\jbr"
set "ANDROID_HOME=D:\Android\Sdk"
set "ANDROID_SDK_ROOT=D:\Android\Sdk"
set "NDK_HOME=D:\Android\Sdk\ndk\27.3.13750724"
set "GRADLE_USER_HOME=D:\Android\gradle-home"
set "PATH=%NDK_HOME%\toolchains\llvm\prebuilt\windows-x86_64\bin;%JAVA_HOME%\bin;%PATH%"
set "CC_aarch64_linux_android=aarch64-linux-android24-clang"
set "AR_aarch64_linux_android=llvm-ar"

if not exist "%JAVA_HOME%\bin\java.exe" (
  echo [ERROR] JAVA_HOME not found: %JAVA_HOME%
  exit /b 1
)
if not exist "%NDK_HOME%" (
  echo [ERROR] NDK not found: %NDK_HOME%
  exit /b 1
)

for /f "delims=" %%v in ('node -p "require('./package.json').version"') do set "VER=%%v"
if "%VER%"=="" (
  echo [ERROR] Cannot read version from package.json
  exit /b 1
)

echo [1/3] Building frontend + Rust + APK (v%VER%)...
call node node_modules\@tauri-apps\cli\tauri.js android build --apk --target aarch64
if errorlevel 1 (
  echo [ERROR] android build failed
  exit /b 1
)

set "APK=src-tauri\gen\android\app\build\outputs\apk\universal\release\app-universal-release.apk"
if not exist "%APK%" (
  echo [ERROR] APK not found: %APK%
  exit /b 1
)

echo [2/3] Copying APK to repository root...
copy /y "%APK%" "RunHelp-v%VER%-android-arm64-signed.apk" >nul
if errorlevel 1 (
  echo [ERROR] copy failed
  exit /b 1
)

echo [3/3] Done: RunHelp-v%VER%-android-arm64-signed.apk
for %%f in ("RunHelp-v%VER%-android-arm64-signed.apk") do echo       %%~zf bytes
exit /b 0
