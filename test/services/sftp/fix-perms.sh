#!/bin/sh
# atmoz/sftp runs /etc/sftp.d/*.sh as root before starting sshd. Docker creates
# the parents of the fixtures bind mount as root, which would leave the chrooted
# user unable to write in its own upload dir, so hand it back here. The mounted
# external/ stays root-owned and read-only, which is fine for reads.
set -e
chown opendal /home/opendal/upload
chmod 755 /home/opendal/upload
