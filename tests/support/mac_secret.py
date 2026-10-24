"""A fake Mac driver for `mac.secret` tests: one running app, one AX node that stays the same, a hidden-input ask
that records each question it was asked (its target and options) and answers a canary, and the apps Launch Services
would list as opening https links (`web`)."""
from types import SimpleNamespace
from typing import Any, Iterable


def secret_driver(bundle: str, name: str | None, asked: list[dict[str, Any]], web: Iterable[str] = ()) -> Any:
    app, node = SimpleNamespace(bundle_id=bundle, name=name, pid=7), SimpleNamespace(ref='field')
    handlers = {w.casefold() for w in web}

    async def ask(ctx: Any, target: str, **options: Any) -> str:
        asked.append({'target': target, **options})
        return 'hidden-canary'
    return SimpleNamespace(apps=SimpleNamespace(find=lambda _: app, opens_web_links=lambda b: b.casefold() in handlers),
                           app=lambda _: app, node=lambda *args: node, ask_secret=ask,
                           ax=SimpleNamespace(set_value=lambda ref, value: True), release_secret=lambda value: None)
