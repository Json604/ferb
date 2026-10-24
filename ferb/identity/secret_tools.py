"""Owns the read-only listing of the owner's saved logins (L36 §5). Never a value, never a Keychain account."""
import asyncio
from pathlib import Path
from typing import Any, Callable

from pydantic import BaseModel, Field

from ferb.core import origin as origins_
from ferb.core.errors import Refused
from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult
from ferb.identity.owner_secrets import OwnerSecrets, login_place


class SecretsList:
    name = 'secrets.list'
    description = ("List what is saved for one login place: the username (plain text, from the owner's own words) and "
                   "the names of its sealed entries (the password, security answers). Never a value. Pass the login "
                   "page's address or origin (https://…), or app:<bundle id>. Call it on a login page before asking "
                   "the owner; fill a listed entry with browser_secret or mac_secret secret=<name>. `linked` lists the "
                   "other login places that serve this site (a sign-in form on another address), each by the place "
                   "its entries fill.")
    scopes, identities = {'task:read'}, {Identity.OWNER}
    risk, reversible, observation, origin = RiskLevel.LOW, True, True, origins_.PRIVATE

    class Input(BaseModel):
        place: str = Field(description="The login page's address or origin (https://…), or app:<bundle id>")

    def __init__(self, home: Callable[[], Path]) -> None:
        self.home = home

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec('read')

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        a: Any = args
        place = login_place(str(a.place))
        if not place:
            return ToolResult({'error': 'give an https:// address or app:<bundle id>'}, is_error=True)
        try:
            listing = await asyncio.to_thread(OwnerSecrets(self.home()).listing, place)
        except Refused as e:                       # an unreadable handle file: say why, change nothing
            return ToolResult({'error': str(e)}, is_error=True)
        # the record is the owner's own data; its entry names are a worker's wording
        return ToolResult(listing, read_sources=(('origin', origins_.PRIVATE), ('origin', origins_.INFERRED)))
