"""Fix wave 3, 8a: FERB keeps its own pinned tokenizer copy under its home. tiktoken's cache in the temporary folder
was emptied by macOS, and every token count (the planner's budget included) silently fell back to a byte ceiling."""
import hashlib
from pathlib import Path

import pytest

from ferb.cli import tokenizer
from ferb.core import context


@pytest.fixture
def pinned(tmp_path, monkeypatch):
    """A small stand-in file, pinned as if it were o200k_base; no cache in the environment or in temp."""
    data = b"stand-in ranks\n"
    monkeypatch.setattr(context, "_ENCODING_SHA256", hashlib.sha256(data).hexdigest())
    monkeypatch.setattr(tokenizer, "TOKENIZER_SHA256", hashlib.sha256(data).hexdigest())
    monkeypatch.delenv("TIKTOKEN_CACHE_DIR", raising=False)
    monkeypatch.delenv("DATA_GYM_CACHE_DIR", raising=False)
    monkeypatch.setattr(context.tempfile, "gettempdir", lambda: str(tmp_path / "temp"))
    monkeypatch.setenv("FERB_HOME", str(tmp_path / "home"))
    return data


def test_ferbs_own_copy_under_its_home_is_found_first(pinned, tmp_path):
    assert context.verified_tokenizer() is None
    own = context.tokenizer_path()
    assert own == tmp_path / "home" / "cache" / "tokenizer" / "o200k_base.tiktoken"
    own.parent.mkdir(parents=True)
    own.write_bytes(pinned)
    assert context.verified_tokenizer() == pinned


def test_a_copy_that_does_not_match_the_pin_is_never_used(pinned):
    own = context.tokenizer_path()
    own.parent.mkdir(parents=True)
    own.write_bytes(b"tampered")
    assert context.verified_tokenizer() is None


def test_setup_copies_a_verified_copy_already_on_this_mac_without_downloading(pinned, tmp_path):
    cache = tmp_path / "temp" / "data-gym-cache"
    cache.mkdir(parents=True)
    (cache / hashlib.sha1(context.TOKENIZER_URL.encode()).hexdigest()).write_bytes(pinned)

    def no_network(url: str) -> bytes:
        raise AssertionError("downloaded")
    said = tokenizer.ensure(tmp_path / "home", fetch=no_network)
    assert "copied" in said and context.tokenizer_path(tmp_path / "home").read_bytes() == pinned
    assert "in place" in tokenizer.ensure(tmp_path / "home", fetch=no_network)


def test_setup_keeps_a_download_only_when_it_matches_the_pin(pinned, tmp_path):
    said = tokenizer.ensure(tmp_path / "home", fetch=lambda url: b"something else")
    assert "doesn't match" in said and not context.tokenizer_path(tmp_path / "home").exists()
    said = tokenizer.ensure(tmp_path / "home", fetch=lambda url: pinned)
    assert "downloaded and verified" in said and context.tokenizer_path(tmp_path / "home").read_bytes() == pinned


def test_setup_never_fails_without_it(pinned, tmp_path):
    def offline(url: str) -> bytes:
        raise OSError("no route")
    assert "byte ceiling" in tokenizer.ensure(tmp_path / "home", fetch=offline)
