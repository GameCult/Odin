@echo off
ssh.exe -o BatchMode=yes -o ConnectTimeout=5 nightwing systemctl is-active --quiet gjallar.service
