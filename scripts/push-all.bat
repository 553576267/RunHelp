@echo off
chcp 65001 >nul
setlocal

rem ============================================================
rem  Fallback mirror push (use when GitCode CI is unavailable).
rem  Fill in the three usernames below, then run this after each commit.
rem  HTTPS remotes will prompt for credentials; use Personal Access
rem  Tokens as the password on GitHub and Gitee.
rem ============================================================

set "GITCODE_USER=binglengdexie"
set "GITHUB_USER=binglengdexie"
set "GITEE_USER=binglengdexie"
set "REPO_NAME=RunHelp"

cd /d "%~dp0.."

echo [1/4] Push to GitCode (main)...
git remote remove gitcode 2>nul
git remote add gitcode "https://gitcode.com/%GITCODE_USER%/%REPO_NAME%.git"
git push gitcode main

echo [2/4] Push to GitHub (main)...
git remote remove github 2>nul
git remote add github "https://github.com/%GITHUB_USER%/%REPO_NAME%.git"
git push github main

echo [3/4] Push to Gitee (master)...
git remote remove gitee 2>nul
git remote add gitee "https://gitee.com/%GITEE_USER%/%REPO_NAME%.git"
git push gitee master

echo [4/4] Push tags to all three...
git push gitcode --tags
git push github --tags
git push gitee --tags

echo.
echo [DONE] All three mirrors updated.
echo.
pause
