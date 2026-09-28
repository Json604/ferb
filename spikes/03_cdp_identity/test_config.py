"""SecretSpecs load only from an owner-only config: allowed origins decide where secrets may go."""

import pytest

from ferb_config import InsecureConfig, load_secret_specs

CONFIG = '[secrets."ferb/x"]\nallowed_origins = ["https://a.example"]\naccount_hint = "x"\n'


@pytest.fixture
def home(tmp_path):
    h = tmp_path / ".ferb"
    h.mkdir(mode=0o700)
    (h / "config.toml").write_text(CONFIG)
    (h / "config.toml").chmod(0o600)
    return h


def test_owner_only_config_loads(home):
    [spec] = load_secret_specs(home)
    assert (spec.ref.key, spec.allowed_origins) == ("ferb/x", ["https://a.example"])


@pytest.mark.parametrize("mode", [0o644, 0o620, 0o604])
def test_config_readable_or_writable_by_others_is_refused(home, mode):
    (home / "config.toml").chmod(mode)
    with pytest.raises(InsecureConfig):
        load_secret_specs(home)


def test_open_directory_is_refused(home):
    home.chmod(0o755)
    with pytest.raises(InsecureConfig):
        load_secret_specs(home)
