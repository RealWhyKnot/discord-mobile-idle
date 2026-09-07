STABLE = "stable"
BETA = "beta"
DEV = "dev"

DIGITS = "0123456789"
HEX = "0123456789ABCDEF"
RANK = {BETA: 0, STABLE: 1}
VISIBLE = {"release": (STABLE,), "beta": (STABLE, BETA)}


def _kind(suffix):
    if not suffix:
        return STABLE
    if suffix in (BETA, DEV):
        return suffix
    if len(suffix) == 4 and not suffix.upper().strip(HEX):
        return DEV
    return None


def parse_version(text):
    if not text:
        return None
    body = text.strip()
    if body[:1] in ("v", "V"):
        body = body[1:]
    head, _, suffix = body.partition("-")
    kind = _kind(suffix.lower())
    if kind is None:
        return None
    parts = head.split(".")
    if len(parts) != 4:
        return None
    numbers = []
    for part in parts:
        if not part or part.strip(DIGITS):
            return None
        numbers.append(int(part))
    return tuple(numbers), kind


def _key(parsed):
    return parsed[0], RANK[parsed[1]]


def select_release(releases, channel, current, skipped=""):
    running = parse_version(current)
    if running is None or running[1] == DEV:
        return None
    visible = VISIBLE.get(channel, ())
    best = None
    best_key = _key(running)
    for release in releases:
        if release.get("draft"):
            continue
        tag = release.get("tag_name", "")
        if skipped and tag == skipped:
            continue
        parsed = parse_version(tag)
        if parsed is None or parsed[1] not in visible:
            continue
        key = _key(parsed)
        if key <= best_key:
            continue
        best = release
        best_key = key
    return best


def asset(release, suffix):
    for item in release.get("assets", ()):
        name = item.get("name", "")
        if name.endswith(suffix):
            return name, item.get("browser_download_url", "")
    return None


def parse_integrity(text, name):
    for line in text.splitlines():
        row = line.strip()
        if not row or row.startswith("#"):
            continue
        fields = row.split("\t")
        if len(fields) != 3 or fields[2] != name:
            continue
        digest = fields[0].strip().lower()
        if len(digest) != 64 or digest.upper().strip(HEX):
            return None
        size = fields[1].strip()
        if not size or size.strip(DIGITS):
            return None
        return digest, int(size)
    return None
