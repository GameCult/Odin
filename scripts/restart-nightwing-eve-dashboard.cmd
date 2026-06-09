@echo off
ssh.exe -o BatchMode=yes -o ConnectTimeout=5 nwroot systemctl restart nightwing-eve-dashboard.service
