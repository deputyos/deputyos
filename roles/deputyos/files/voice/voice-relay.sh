#!/usr/bin/env bash
# /opt/deputyos/voice/voice-relay.sh — the audio-side bridge for the
# Phase 7 Lane Voice (M6) feature.
#
# Wire-up:
#
#    +-----------+ alsa  +-------------+ json   +-------------+
#    |   mic     |------>|  whisper-cli|------->|  this script|
#    +-----------+       +-------------+        +------+------+
#                                                      |
#                                       wake-word check (substring)
#                                                      |
#                                                      v
#                              {"kind":"pre-message",      +-------------+
#                               "payload":{"text":"...",   |             |
#                               "source":"voice"}}        |  deputyctl   |
#                                                          | message-    |
#                                                +-------->|   relay     |
#                                                |         +------+------+
#                                                |                |
#                                  /run/deputyos/relay.sock        |
#                                                |   reply text   |
#                                                +<---------------+
#                                                |
#                                                v
#                                         +-------------+
#                                         |   piper     |
#                                         +------+------+
#                                                | raw PCM
#                                                v
#                                         +-------------+
#                                         |    aplay    | -> speaker
#                                         +-------------+
#
# Reference: docs/11-roadmap.md §M6 Lane A/B/F,
#            deputyctl/src/message_relay.rs (wire protocol).
#
# IMPORTANT — HookKind invariant:
#   Lane M7 owns crates::hooks::HookKind. We do NOT introduce a
#   "VoiceInput" variant (the four-variant set pre/post-message,
#   cost-alert, update-applied is stable). Voice events are framed as
#   `kind: "pre-message"` with `source: "voice"` in the payload — a
#   convention encoded in /etc/deputyos/voice.toml [relay] as well.

set -euo pipefail

CONFIG="${DEPUTYOS_VOICE_CONFIG:-/etc/deputyos/voice.toml}"

# ---------- Tiny TOML reader (string keys only, no nesting madness) -------
# /etc/deputyos/voice.toml is a flat-ish KV file we render ourselves; we
# don't need a real parser. Returns the value (unquoted) for the first
# match of `key =` after the optional `[section]` header.
#
# The awk program is fed via env vars and `-f -` to keep shellcheck from
# mis-parsing the embedded apostrophes/brackets as shell quoting.
toml_get() {
    local section="$1" key="$2" file="$3"
    DEPUTYOS_AWK_SECTION="[$section]" DEPUTYOS_AWK_KEY="$key" \
        awk -f /dev/stdin "$file" <<'AWK_EOF'
BEGIN {
    section = ENVIRON["DEPUTYOS_AWK_SECTION"]
    key     = ENVIRON["DEPUTYOS_AWK_KEY"]
    in_section = (section == "[]")
}
/^\[.*\]/ {
    in_section = ($0 == section)
    next
}
{
    if (!in_section) next
    if ($0 ~ "^[[:space:]]*" key "[[:space:]]*=") {
        sub("^[^=]*=[[:space:]]*", "", $0)
        sub("[[:space:]]*$", "", $0)
        # Strip surrounding double quotes, then surrounding single quotes.
        sub("^\"", "", $0); sub("\"$", "", $0)
        sub("^'\''", "", $0); sub("'\''$", "", $0)
        print
        exit
    }
}
AWK_EOF
}

# ---------- Bootstrap config ---------------------------------------------
if [[ ! -r "$CONFIG" ]]; then
    echo "voice-relay: cannot read $CONFIG" >&2
    exit 64
fi

VOICE_ENABLED="$(toml_get voice enabled "$CONFIG")"
WAKE_WORD="$(toml_get voice wake_word "$CONFIG")"
STT_MODEL_PATH="$(toml_get stt model_path "$CONFIG")"
STT_BINARY="$(toml_get stt binary "$CONFIG")"
TTS_MODEL_PATH="$(toml_get tts model_path "$CONFIG")"
TTS_BINARY="$(toml_get tts binary "$CONFIG")"
AUDIO_DEVICE="$(toml_get voice audio_device "$CONFIG")"
RELAY_SOCKET="$(toml_get relay socket_path "$CONFIG")"
HOOK_KIND="$(toml_get relay hook_kind "$CONFIG")"
SOURCE_TAG="$(toml_get relay source_tag "$CONFIG")"

: "${VOICE_ENABLED:=false}"
: "${WAKE_WORD:=agent}"
: "${AUDIO_DEVICE:=default}"
: "${RELAY_SOCKET:=/run/deputyos/relay.sock}"
: "${HOOK_KIND:=pre-message}"
: "${SOURCE_TAG:=voice}"
VOICE_REPLY_SOCKET="$(toml_get relay voice_reply_socket "$CONFIG")"
: "${VOICE_REPLY_SOCKET:=/run/deputyos/voice-reply.sock}"

if [[ "$VOICE_ENABLED" != "true" ]]; then
    echo "voice-relay: disabled in $CONFIG (voice.enabled=false); exiting" >&2
    exit 0
fi

for path in "$STT_BINARY" "$STT_MODEL_PATH" "$TTS_BINARY" "$TTS_MODEL_PATH"; do
    if [[ ! -e "$path" ]]; then
        echo "voice-relay: required asset missing: $path" >&2
        echo "  rerun the bake with voice assets staged (scripts/build.sh)" >&2
        exit 65
    fi
done

if [[ ! -S "$RELAY_SOCKET" ]]; then
    echo "voice-relay: relay socket $RELAY_SOCKET not present yet — sleeping 30s" >&2
    sleep 30
    if [[ ! -S "$RELAY_SOCKET" ]]; then
        echo "voice-relay: relay socket still missing; exiting (systemd will Restart=on-failure)" >&2
        exit 69
    fi
fi

# ---------- Wake-word match (case-insensitive substring on first token) --
# Returns 0 + prints the post-wake-word remainder when matched, 1 otherwise.
wake_word_strip() {
    local text="$1" ww
    ww="$(echo "$WAKE_WORD" | tr '[:upper:]' '[:lower:]')"
    local lower
    lower="$(echo "$text" | tr '[:upper:]' '[:lower:]')"
    if [[ "$lower" == "$ww"* || "$lower" == *" $ww "* ]]; then
        # Strip up to and including the wake word (case-preserving).
        echo "$text" | sed -E "s/^.*[[:space:]]?${WAKE_WORD}[[:space:]]?//I" \
            | sed -E "s/^[[:space:]]+//"
        return 0
    fi
    return 1
}

# ---------- Main capture/dispatch loop -----------------------------------
# whisper-cli's --output-json + --silence-detect emit one JSON object per
# detected utterance to stdout. We parse `text` from each line, run the
# wake-word filter, and dispatch matches to the relay. The script blocks
# in the while-read; whisper-cli is the long-running child.
echo "voice-relay: starting (model=${STT_MODEL_PATH##*/}, device=${AUDIO_DEVICE}, wake=\"$WAKE_WORD\")"

dispatch_to_relay() {
    local text="$1" payload reply
    payload="$(jq -cn \
        --arg kind "$HOOK_KIND" \
        --arg text "$text" \
        --arg source "$SOURCE_TAG" \
        '{kind:$kind, payload:{text:$text, source:$source}}')"
    # nc -U writes the request, half-closes (-N), and prints the hook status.
    reply="$(printf '%s\n' "$payload" | nc -U -N "$RELAY_SOCKET" 2>/dev/null || true)"

    # The agent's reply text arrives on the voice-reply Unix datagram socket
    # (/run/deputyos/voice-reply.sock). Block up to 5s for the reply.
    local spoken=""
    if [[ -S "$VOICE_REPLY_SOCKET" ]]; then
        spoken="$(timeout 5 nc -U -l "$VOICE_REPLY_SOCKET" 2>/dev/null || true)"
    fi

    # Fallback: if the agent didn't reply in time, echo what we heard.
    if [[ -z "$spoken" ]]; then
        spoken="$text"
    fi
    speak "$spoken"
}

speak() {
    local text="$1"
    # piper reads text from stdin, writes raw 16-bit signed-LE PCM @
    # 22050 Hz to stdout (default for the amy-medium voice). aplay
    # consumes that directly.
    printf '%s\n' "$text" \
        | "$TTS_BINARY" \
            --model "$TTS_MODEL_PATH" \
            --output-raw 2>/dev/null \
        | aplay -D "$AUDIO_DEVICE" -q -r 22050 -f S16_LE -t raw -
}

# Long-running whisper-cli with continuous capture. The -nt flag drops
# non-speech segments and -np avoids prefix/colour escape sequences in
# the JSON path. --silence-detect chunks utterances at silence rather
# than fixed windows so the wake-word + payload arrive together.
"$STT_BINARY" \
    --model "$STT_MODEL_PATH" \
    --output-json /dev/stdout \
    --silence-detect \
    --device "$AUDIO_DEVICE" \
    -nt -np \
    2>/dev/null \
    | while IFS= read -r line; do
        # Only process lines that look like JSON objects with a text field.
        if [[ "$line" != *'"text"'* ]]; then
            continue
        fi
        text="$(printf '%s' "$line" | jq -r '.text // empty' 2>/dev/null || true)"
        if [[ -z "$text" ]]; then
            continue
        fi
        # Strip leading/trailing whitespace.
        text="${text#"${text%%[![:space:]]*}"}"
        text="${text%"${text##*[![:space:]]}"}"
        if [[ -z "$text" ]]; then
            continue
        fi
        if remainder="$(wake_word_strip "$text")"; then
            if [[ -z "$remainder" ]]; then
                # Wake word with no payload — acknowledge.
                speak "yes?"
                continue
            fi
            echo "voice-relay: dispatching: $remainder"
            dispatch_to_relay "$remainder"
        fi
    done
