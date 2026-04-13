#!/usr/bin/env bash
# One-time server-side bootstrap so CI can deploy apache config without sudo.
#
# Run as root on the prod host (e.g. `sudo bash bootstrap.sh`). Idempotent.
#
# What it sets up:
#   1. `apache-deploy` group; ci user is added to it.
#   2. The two managed apache files are chgrped + chmoded so members of
#      the group can edit them in place. We do NOT make the directories
#      group-writable -- ci can update existing files but cannot create
#      sibling files in /etc/apache2/.
#   3. systemd path unit that watches the two files and triggers a
#      validate-then-reload when either changes. ci never needs to call
#      systemctl or sudo -- editing the file is enough.
#   4. Secret directory exists with right perms (file content is still
#      installed by hand by an admin who has the secret).

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "must run as root" >&2
    exit 1
fi

CI_USER=${CI_USER:-ci}
DEPLOY_GROUP=apache-deploy

VHOST=/etc/apache2/sites-enabled/minerva-app.conf
LUA=/etc/apache2/lua/minerva-ext-auth.lua
SECRET_DIR=/etc/apache2/secrets

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)

echo "==> ensuring group $DEPLOY_GROUP and adding $CI_USER to it"
getent group "$DEPLOY_GROUP" >/dev/null || groupadd "$DEPLOY_GROUP"
usermod -aG "$DEPLOY_GROUP" "$CI_USER"

echo "==> ensuring $LUA exists with deploy-group write"
install -d -m 0755 /etc/apache2/lua
touch "$LUA"
chgrp "$DEPLOY_GROUP" "$LUA"
chmod 0664 "$LUA"

echo "==> ensuring $VHOST exists with deploy-group write"
touch "$VHOST"
chgrp "$DEPLOY_GROUP" "$VHOST"
chmod 0664 "$VHOST"

echo "==> ensuring $SECRET_DIR (root:www-data, 0750)"
install -d -m 0750 -o root -g www-data "$SECRET_DIR"
if [[ ! -f $SECRET_DIR/minerva-hmac ]]; then
    echo "    NOTE: $SECRET_DIR/minerva-hmac is missing." >&2
    echo "    Install from k8s:" >&2
    echo "      kubectl get secret -n minerva minerva-secrets -o jsonpath='{.data.MINERVA_HMAC_SECRET}' \\" >&2
    echo "        | base64 -d > $SECRET_DIR/minerva-hmac && chown root:www-data $SECRET_DIR/minerva-hmac && chmod 0640 $SECRET_DIR/minerva-hmac" >&2
fi

echo "==> installing systemd path + service for auto-reload"
install -m 0644 -o root -g root "$SCRIPT_DIR/minerva-apache-reload.path" /etc/systemd/system/
install -m 0644 -o root -g root "$SCRIPT_DIR/minerva-apache-reload.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now minerva-apache-reload.path
# Reset failure counter from any previous run.
systemctl reset-failed minerva-apache-reload.service 2>/dev/null || true

echo "==> enabling mod_lua / mod_rewrite / mod_headers (idempotent)"
a2enmod lua rewrite headers >/dev/null

echo
echo "Bootstrap complete."
echo "From now on, ci can edit $VHOST and $LUA directly; systemd will"
echo "validate the new config and reload apache automatically. Watch with:"
echo "  journalctl -u minerva-apache-reload -f"
