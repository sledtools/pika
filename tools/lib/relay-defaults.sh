#!/usr/bin/env bash
set -euo pipefail

# Shared relay defaults for local tooling scripts.

default_message_relays_csv() {
  echo "wss://relay.primal.net,wss://nos.lol,wss://relay.damus.io,wss://us-east.nostr.pikachat.org,wss://eu.nostr.pikachat.org"
}

default_key_package_relays_csv() {
  echo "wss://nostr-pub.wellorder.net,wss://nostr-01.yakihonne.com,wss://nostr-02.yakihonne.com"
}
