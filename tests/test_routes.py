"""The environment line (section 9.3) and what's left of the service catalogue: the supervisor is told which named
services have an app installed and a website, and which apps are running — and decides the route itself.
Nothing here refuses anything.
"""

from ferb.policy import services as svc
from ferb.policy.services import Environment

WA_APP = ("net.whatsapp.WhatsApp", "WhatsApp")
BRAVE_WORK = ("brave", "Default", "Work")


def env(**kw: object) -> Environment:
    base: dict[str, object] = {"apps": [WA_APP, ("com.apple.Notes", "Notes")], "paired": [BRAVE_WORK]}
    return Environment(**(base | kw))   # type: ignore[arg-type]


def test_services_come_from_the_catalogue_and_installed_apps() -> None:
    e = env(apps=[WA_APP, ("com.culturedcode.ThingsMac", "Things")])
    names = [s.name for s in svc.services_named("video call Rahul on whatsapp, then add it to things", e)]
    assert names == ["WhatsApp", "Things"]
    assert svc.services_named("what's the weather", e) == []
    assert [s.name for s in svc.services_named("call aditya in wa", e)] == ["WhatsApp"]        # an alias


def test_the_environment_line_says_what_is_installed_paired_and_running() -> None:
    e = env()
    named = svc.services_named("message Rahul on WhatsApp and note it in Notes", e)
    stable, live = svc.summary(e, "26.0", named, {"net.whatsapp.WhatsApp"})
    assert stable.startswith("This Mac: macOS 26.0. Browser profiles FERB can use: Brave “Work”.")
    assert "WhatsApp: app WhatsApp; web web.whatsapp.com" in stable and "Notes: app Notes" in stable
    assert live == "Running now: WhatsApp."
    assert svc.summary(env(paired=[]), "26.0", [], set())[0].endswith("none paired.")


def test_communication_apps_are_the_catalogues_messaging_and_calling_apps() -> None:
    bundles = svc.communication_bundles()
    assert "net.whatsapp.whatsapp" in bundles and "com.tinyspeck.slackmacgap" in bundles
    assert "com.apple.notes" not in bundles and "com.spotify.client" not in bundles
