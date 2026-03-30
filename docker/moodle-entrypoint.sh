#!/bin/bash
set -e

MOODLE_VER="MOODLE_405_STABLE"
WWWROOT="/var/www/html"
DATAROOT="/var/www/moodledata"

# Wait for DB
echo "Waiting for database..."
until php -r "@pg_connect('host=moodle-db dbname=moodle user=moodle password=moodle') or exit(1);" 2>/dev/null; do
    sleep 2
done
echo "Database is ready."

# Download Moodle if not already present
if [ ! -f "$WWWROOT/version.php" ]; then
    echo "Downloading Moodle ($MOODLE_VER)..."
    apt-get update -qq && apt-get install -y -qq curl > /dev/null 2>&1
    cd /tmp
    curl -Lo moodle.tar.gz "https://github.com/moodle/moodle/archive/refs/heads/$MOODLE_VER.tar.gz"
    tar xzf moodle.tar.gz --strip-components=1 -C "$WWWROOT"
    rm moodle.tar.gz
    echo "Moodle downloaded."
fi

# Ensure moodledata exists and is writable
mkdir -p "$DATAROOT"
chown -R www-data:www-data "$DATAROOT"
chmod -R 0777 "$DATAROOT"

# Create config.php if missing
if [ ! -f "$WWWROOT/config.php" ]; then
    echo "Running Moodle CLI install..."
    php "$WWWROOT/admin/cli/install.php" \
        --wwwroot="http://localhost:8088" \
        --dataroot="$DATAROOT" \
        --dbtype=pgsql \
        --dbhost=moodle-db \
        --dbname=moodle \
        --dbuser=moodle \
        --dbpass=moodle \
        --fullname="Minerva Dev" \
        --shortname="minervadev" \
        --adminuser=admin \
        --adminpass="Admin123!" \
        --adminemail="admin@localhost.test" \
        --agree-license \
        --non-interactive
    echo "Moodle installed."
fi

# Dev: allow outgoing HTTP to Docker-internal hosts on any port.
if ! grep -q 'curlsecurityblockedhosts' "$WWWROOT/config.php"; then
    sed -i "/require_once.*lib\/setup.php/i \\
// Dev overrides: allow curl to Docker-internal hosts.\\
\\\$CFG->curlsecurityblockedhosts = '';\\
\\\$CFG->curlsecurityallowedport = '';" "$WWWROOT/config.php"
    echo "Curl security relaxed for dev."
fi

# Fix permissions for the web server
chown -R www-data:www-data "$WWWROOT"

echo "Starting Apache..."
exec apache2-foreground
