#!/usr/bin/env python3
"""Refuse a deploy that would drop a secret key the cluster still has.

Every deploy re-applies the whole `K8S_SECRETS` that Terraform generates,
so a key that is set on the cluster but missing (or empty) in Terraform's
output is wiped by the next deploy, and the feature behind it switches off
without an error: an empty provider key just leaves that provider
unregistered. This compares the two before `kubectl apply` and fails on
any key that is non-empty on the cluster and would be missing or empty
afterwards, unless the key is named in `--allow` (the deploy workflow's
`allow_secret_removal` input).

Only key names are printed, never values.

Usage:
    check_secret_removal.py NEW_JSON LIVE_JSON [--allow KEY[,KEY...]]

NEW_JSON is `kubectl create --dry-run=client -o json -f secrets.yaml` (one
object per document, back to back), LIVE_JSON is `kubectl get secret -n
minerva -o json` for the same names (a List, or empty if none exist yet).
"""

import argparse
import base64
import json
import sys


def load_objects(text: str) -> list[dict]:
    """Every object in `text`, which kubectl writes as one JSON document
    per object back to back (a multi-document manifest), or as a List.
    """
    decoder = json.JSONDecoder()
    objects = []
    pos = 0
    text = text.strip()
    while pos < len(text):
        doc, pos = decoder.raw_decode(text, pos)
        while pos < len(text) and text[pos].isspace():
            pos += 1
        objects.extend(doc.get("items", []) if "items" in doc else [doc])
    return objects


def filled_keys(objects: list[dict]) -> dict[str, set[str]]:
    """Secret name -> keys whose value is non-empty, from either form."""
    out: dict[str, set[str]] = {}
    for item in objects:
        if item.get("kind") != "Secret":
            continue
        keys = out.setdefault(item["metadata"]["name"], set())
        for key, value in (item.get("stringData") or {}).items():
            if value:
                keys.add(key)
        for key, value in (item.get("data") or {}).items():
            if value and base64.b64decode(value):
                keys.add(key)
    return out


def lost_keys(new: dict[str, set[str]], live: dict[str, set[str]]) -> list[str]:
    """`secret/KEY` for every key the deploy would empty or remove.

    Only secrets the new manifest declares are compared: a secret it does
    not mention is not touched by the apply.
    """
    lost = []
    for name, new_keys in new.items():
        for key in sorted(live.get(name, set()) - new_keys):
            lost.append(f"{name}/{key}")
    return lost


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("new")
    parser.add_argument("live")
    parser.add_argument("--allow", default="")
    args = parser.parse_args(argv)

    with open(args.new) as f:
        new = filled_keys(load_objects(f.read()))
    with open(args.live) as f:
        live = filled_keys(load_objects(f.read()))
    allowed = {k.strip() for k in args.allow.split(",") if k.strip()}

    lost = lost_keys(new, live)
    blocked = [k for k in lost if k not in allowed and k.split("/", 1)[1] not in allowed]
    for key in lost:
        if key not in blocked:
            print(f"allowed: {key} is removed or emptied by this deploy")
    if blocked:
        print("These secret keys are set on the cluster but missing or empty in K8S_SECRETS:")
        for key in blocked:
            print(f"  {key}")
        print(
            "Add them to terraform/terraform.tfvars and run `terraform apply`, "
            "or rerun the deploy with allow_secret_removal naming them if the "
            "removal is intended."
        )
        return 1
    print(f"secret keys checked: {sum(len(v) for v in new.values())}, none dropped without approval")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
