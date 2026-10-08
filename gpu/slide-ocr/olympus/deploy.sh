#!/bin/bash
# Run on Olympus by the deploy-slide-ocr workflow (ssh ... 'bash -s' <sha>),
# after it has uploaded the code as ~/minerva-slide-ocr/.incoming.tgz. Puts
# the files in place; state (venv/, logs/, grants/, work/, olympus/site.env)
# is never touched. The first deploy creates the directory.

set -euo pipefail
sha=${1:?usage: deploy.sh <git-sha>}
cd "$HOME/minerva-slide-ocr"
tmp=$(mktemp -d .deploy.XXXXXX)
trap 'rm -r "$tmp" .incoming.tgz' EXIT
tar -xzf .incoming.tgz -C "$tmp"
# Rename each file into place: running scripts keep their old inode, where
# an in-place overwrite could change a script under bash's feet.
(cd "$tmp" && find . -type f) | while read -r f; do
    f=${f#./}
    if [[ "$f" =~ ^(venv|logs|grants|work)/ || "$f" = olympus/site.env ]]; then
        continue
    fi
    mkdir -p "$(dirname "$f")"
    mv -f "$tmp/$f" "$f"
done
echo "$sha" > DEPLOYED
echo "deployed $sha"
