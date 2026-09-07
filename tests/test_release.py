import pytest

from idlebot.release import BETA, DEV, STABLE, asset, parse_integrity, parse_version, select_release


def rel(tag, draft=False, assets=None):
    return {"tag_name": tag, "draft": draft, "assets": assets or []}


@pytest.mark.parametrize(
    "text, expected",
    [
        ("2026.9.7.0", ((2026, 9, 7, 0), STABLE)),
        ("v2026.9.7.0", ((2026, 9, 7, 0), STABLE)),
        ("V2026.9.7.0", ((2026, 9, 7, 0), STABLE)),
        ("  v2026.9.7.0  ", ((2026, 9, 7, 0), STABLE)),
        ("v2026.9.7.0-beta", ((2026, 9, 7, 0), BETA)),
        ("v2026.9.7.0-BETA", ((2026, 9, 7, 0), BETA)),
        ("v2026.9.7.0-A528", ((2026, 9, 7, 0), DEV)),
        ("2026.9.7.0-a528", ((2026, 9, 7, 0), DEV)),
        ("0.0.0.0-dev", ((0, 0, 0, 0), DEV)),
    ],
)
def test_parse_version_accepts(text, expected):
    assert parse_version(text) == expected


@pytest.mark.parametrize(
    "text",
    ["", "   ", "v2026.9.7", "v2026.9.7.0.1", "v2026..7.0", "v2026.x.7.0", "v2026.9.7.0-ZZZZ", "v2026.9.7.0-alpha"],
)
def test_parse_version_rejects(text):
    assert parse_version(text) is None


def test_a_beta_tag_never_raises_and_is_hidden_from_release():
    releases = [rel("v2026.9.8.0-beta")]

    assert select_release(releases, "release", "2026.9.7.0") is None
    assert select_release(releases, "beta", "2026.9.7.0") is releases[0]


@pytest.mark.parametrize("channel", ["release", "beta"])
def test_a_hex_stamped_tag_is_never_offered(channel):
    assert select_release([rel("v2026.9.8.0-A528")], channel, "2026.9.7.0") is None


def test_newer_stable_is_offered():
    releases = [rel("v2026.9.8.0")]
    assert select_release(releases, "release", "2026.9.7.0") is releases[0]


@pytest.mark.parametrize("current", ["2026.9.8.0", "2026.9.9.0"])
def test_same_or_older_is_not_offered(current):
    assert select_release([rel("v2026.9.8.0")], "release", current) is None


@pytest.mark.parametrize("current", ["not-a-version", "0.0.0.0-dev", "2026.9.7.0-A528"])
def test_an_unusable_running_version_checks_nothing(current):
    assert select_release([rel("v2026.9.8.0")], "release", current) is None


@pytest.mark.parametrize("channel", ["dev", "", "nonsense"])
def test_an_unknown_channel_sees_nothing(channel):
    assert select_release([rel("v2026.9.8.0")], channel, "2026.9.7.0") is None


def test_drafts_are_skipped():
    assert select_release([rel("v2026.9.8.0", draft=True)], "release", "2026.9.7.0") is None


def test_the_skipped_tag_is_suppressed():
    releases = [rel("v2026.9.8.0")]

    assert select_release(releases, "release", "2026.9.7.0", skipped="v2026.9.8.0") is None
    assert select_release(releases, "release", "2026.9.7.0", skipped="v2026.9.9.0") is releases[0]


@pytest.mark.parametrize("order", [(0, 1), (1, 0)])
def test_a_tie_prefers_the_stable(order):
    pair = [rel("v2026.9.8.0"), rel("v2026.9.8.0-beta")]
    releases = [pair[order[0]], pair[order[1]]]

    assert select_release(releases, "beta", "2026.9.7.0") is pair[0]


def test_the_highest_wins_regardless_of_order():
    releases = [rel("v2026.9.9.0"), rel("v2026.9.8.0")]
    assert select_release(releases, "release", "2026.9.7.0") is releases[0]


def test_unparseable_tags_are_ignored():
    releases = [rel("nightly"), rel("v2026.9.8.0")]
    assert select_release(releases, "release", "2026.9.7.0") is releases[1]


def test_no_releases_at_all():
    assert select_release([], "release", "2026.9.7.0") is None


def test_asset_matches_by_suffix():
    release = rel(
        "v2026.9.8.0",
        assets=[
            {"name": "a.integrity.tsv", "browser_download_url": "https://x/tsv"},
            {"name": "a.zip", "browser_download_url": "https://x/zip"},
        ],
    )

    assert asset(release, ".zip") == ("a.zip", "https://x/zip")
    assert asset(release, ".integrity.tsv") == ("a.integrity.tsv", "https://x/tsv")
    assert asset(release, ".exe") is None


def test_asset_tolerates_a_bare_release():
    assert asset(rel("v2026.9.8.0"), ".zip") is None
    assert asset(rel("v2026.9.8.0", assets=[{"name": "a.zip"}]), ".zip") == ("a.zip", "")


def test_parse_integrity_reads_the_matching_row():
    text = "# comment\n\n%s\t123\tother.zip\n%s\t456\ta.zip\n" % ("a" * 64, "B" * 64)

    assert parse_integrity(text, "a.zip") == ("b" * 64, 456)


def test_parse_integrity_tolerates_crlf():
    assert parse_integrity("%s\t7\ta.zip\r\n" % ("a" * 64), "a.zip") == ("a" * 64, 7)


@pytest.mark.parametrize(
    "text",
    [
        "",
        "%s\t9\tother.zip\n" % ("a" * 64),
        "%s\t9\n" % ("a" * 64),
        "%s\t9\ta.zip\textra\n" % ("a" * 64),
    ],
)
def test_parse_integrity_finds_nothing(text):
    assert parse_integrity(text, "a.zip") is None


@pytest.mark.parametrize(
    "digest, size",
    [("a" * 63, "9"), ("z" * 64, "9"), ("a" * 64, ""), ("a" * 64, "9x")],
)
def test_parse_integrity_rejects_a_bad_row(digest, size):
    assert parse_integrity("%s\t%s\ta.zip\n" % (digest, size), "a.zip") is None
