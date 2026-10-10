"""P21 owner-written skills, with a small catalog and confined, explicit body reads."""
import os
from pathlib import Path
from typing import Any

from pydantic import BaseModel

from ferb.core.errors import Refused
from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult


class Skills:
    def __init__(self, directory: Path) -> None:
        self.directory = directory

    def _read(self, skill: str, relative: str) -> str:
        parts = [skill, *Path(relative).parts]
        if not skill or Path(skill).name != skill or any(p in ('', '.', '..') or '/' in p for p in parts):
            raise Refused('Choose a skill and a file inside its folder.')
        descriptors: list[int] = []
        try:
            # Each component is opened relative to an already-open directory; no symlink substitution can leave it.
            descriptors.append(os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW))
            for part in parts[:-1]:
                descriptors.append(os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                           dir_fd=descriptors[-1]))
            fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW, dir_fd=descriptors[-1])
            with os.fdopen(fd, 'r', encoding='utf-8') as file:
                return file.read()
        except (OSError, UnicodeError) as exc:
            raise Refused('That skill file is unavailable or is a link outside its folder.') from exc
        finally:
            for fd in reversed(descriptors):
                os.close(fd)

    @staticmethod
    def _parse(text: str) -> tuple[dict[str, str], str]:
        if not text.startswith('---\n'):
            return {}, text
        head, separator, body = text[4:].partition('\n---\n')
        if not separator:
            return {}, text
        fields = {}
        for line in head.splitlines():
            key, sep, value = line.partition(':')
            if sep and key in ('name', 'description'):
                fields[key] = value.strip().strip('"\'')
        return fields, body

    def catalog(self) -> list[dict[str, str]]:
        if not self.directory.is_dir() or self.directory.is_symlink():
            return []
        items = []
        for path in sorted(self.directory.iterdir()):
            try:
                fields, _ = self._parse(self._read(path.name, 'SKILL.md'))
            except Refused:
                continue
            if fields.get('name') and fields.get('description'):
                items.append({'id': path.name, **fields})
        return items

    def read(self, skill: str, file: str = 'SKILL.md') -> dict[str, Any]:
        text = self._read(skill, file)
        fields, body = self._parse(text) if file == 'SKILL.md' else ({}, text)
        return {'id': skill, 'file': file, **fields, 'body': body}


class SkillsRead:
    name = 'skills.read'
    description = 'Read an owner-written skill from the catalog, or a relative supporting file in that skill folder.'
    scopes, identities = {'skills:read'}, {Identity.OWNER, Identity.FERB}
    risk, reversible, observation, origin = RiskLevel.LOW, True, True, 'owner'

    class Input(BaseModel):
        skill: str
        file: str = 'SKILL.md'

    def __init__(self, skills: Skills) -> None:
        self.skills = skills

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec('read')

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        a: Any = args
        return ToolResult(self.skills.read(a.skill, a.file))
