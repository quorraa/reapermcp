"""Install a Surge factory .fxp by writing it as the plugin's VST3 state.

TrackFX_SetPreset refuses .fxp on a VST3, but the payload inside an .fxp and the
payload inside Surge's VST3 state are the same 'sub3' patch blob. So the patch
can be installed by rebuilding the state around the factory blob.

Layout, established by inspection rather than from documentation:

    VST3 state : [uint32le size][uint32le version=1] <sub3 blob> [trailer]
    .fxp       : VST2 'CcnK'/'FPCh' header, big-endian, payload at offset 60

The trailer carried in the live state is Surge's DAW-extra-state; it is kept as
found, since it belongs to the instance rather than to the patch.
"""
import base64
import struct

FXP_PAYLOAD_OFFSET = 60
SUB3_HEADER = 32          # 'sub3' + uint32 size + 24 reserved bytes


def fxp_payload(path):
    """The 'sub3' patch blob carried inside a VST2 .fxp file."""
    with open(path, "rb") as fh:
        f = fh.read()
    if f[:4] != b"CcnK" or f[8:12] != b"FPCh":
        raise ValueError("%s is not an opaque-chunk .fxp" % path)
    size = struct.unpack(">I", f[56:60])[0]
    payload = f[FXP_PAYLOAD_OFFSET:FXP_PAYLOAD_OFFSET + size]
    if payload[:4] != b"sub3":
        raise ValueError("%s payload is not a Surge patch" % path)
    return payload


def split_state(blob):
    """-> (version, sub3 blob, trailer) for a live Surge VST3 state."""
    declared, version = struct.unpack("<II", blob[:8])
    body = blob[8:]
    if body[:4] != b"sub3":
        raise ValueError("state does not start with a Surge patch")
    inner = struct.unpack("<I", body[4:8])[0]
    end = SUB3_HEADER + inner
    return version, body[:end], body[end:], declared


def build_state(live_blob, patch_payload):
    """Live state with its patch replaced by `patch_payload`."""
    version, _old, trailer, declared = split_state(live_blob)
    body = patch_payload + trailer
    # The live state's length field ran 8 short of the body it carried; keep
    # that relationship rather than inventing one.
    slack = len(live_blob) - 8 - declared
    return struct.pack("<II", len(body) - slack, version) + body


def describe(blob):
    version, patch, trailer, declared = split_state(blob)
    return ("version=%d declared=%d total=%d patch=%d trailer=%d"
            % (version, declared, len(blob), len(patch), len(trailer)))


def patch_name(blob):
    """Pull the patch name out of the XML inside a state blob."""
    i = blob.find(b"<patch")
    if i < 0:
        return None
    seg = blob[i:i + 400].decode("utf-8", "replace")
    for key in ('name="', "name='"):
        j = seg.find(key)
        if j >= 0:
            j += len(key)
            return seg[j:seg.find(seg[j - 1], j)]
    return None


def b64(blob):
    return base64.b64encode(blob).decode("ascii")
