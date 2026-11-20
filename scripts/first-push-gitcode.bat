@echo off
chcp 65001 >nul
setlocal

rem ============================================================
rem  First push RunHelp to GitCode.
rem  Before running:
rem    1. Create an EMPTY public repo named RunHelp on gitcode.com
rem       (do NOT add README, .gitignore or license)
rem    2. Create a Personal Access Token on GitCode with repo write permission
rem    3. GitCode username is already filled in below (binglengdexie)
rem ============================================================

set "GITCODE_USER=binglengdexie"
set "REPO_NAME=RunHelp"

if "%GITCODE_USER%"=="" (
  echo [ERROR] GITCODE_USER is empty. Please fill in your GitCode username.
  pause
  exit /b 1
)

cd /d "%~dp0.."

echo [1/6] Check git identity...
git config user.name >nul 2>nul
if errorlevel 1 (
  echo [ERROR] Git identity not set. Run these two commands first:
  echo   git config --global user.name "Your Name"
  echo   git config --global user.email "you@example.com"
  pause
  exit /b 1
)

echo [2/6] Init repo...
git init

echo [3/6] Add remote...
git remote remove origin 2>nul
git remote add origin "https://gitcode.com/%GITCODE_USER%/%REPO_NAME%.git"

echo [4/6] Stage files...
git add .

echo [5/6] Commit...
git commit -m "initial commit"

echo [6/6] Push to GitCode (username=GitCode account, password=Personal Access Token)...
git branch -M main
git push -u origin main

echo.
echo [DONE] If push succeeded, configure the four secrets in the GitCode repo Settings.
echo.
pause
