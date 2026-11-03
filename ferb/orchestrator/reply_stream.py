"""Provisional public planner text, written last after validated metadata.

Pure conversational answers keep their reply channel. Task plans use a separate
pre-worker channel; raw structured fragments stay in memory. Actions and reordered
responses remain final-only. Malformed drafts are withdrawn; worker reports and
reasoning never enter this path.
"""
import json
import unicodedata
from typing import Any, Callable

from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA, SupervisorOutcome
from ferb.policy.confirm import from_task
from ferb.sanitize.redact import Redactor


class _LineGuard:
    """Apply D33 before publishing any prefix that could impersonate FERB."""

    def __init__(self) -> None:
        self.prefix = ''
        self.started = False

    def feed(self, text: str, final: bool = False) -> str:
        out = []
        for part in text.splitlines(keepends=True):
            if self.started:
                out.append(part)
            else:
                self.prefix += part
                letters = ''.join(c for c in unicodedata.normalize('NFKC', self.prefix) if c.isalnum())
                if len(letters) >= 4 or '\n' in part:
                    out.append(from_task(self.prefix))
                    self.prefix = ''
                    self.started = True
            if part.endswith('\n'):
                self.started = False
        if final and self.prefix:
            out.append(from_task(self.prefix))
            self.prefix = ''
        return ''.join(out)


def _character(raw: str, pos: int) -> tuple[str, int] | None:
    char = raw[pos]
    if char != '\\':
        if ord(char) < 32 or 0xD800 <= ord(char) <= 0xDFFF:
            raise ValueError('invalid JSON string character')
        return char, 1
    if len(raw) < pos + 2:
        return None
    size = 6 if raw[pos + 1] == 'u' else 2
    if len(raw) < pos + size:
        return None
    value = json.loads('"' + raw[pos:pos + size] + '"')
    if len(value) == 1 and 0xD800 <= ord(value) <= 0xDBFF:
        if len(raw) < pos + 12:
            return None
        low = raw[pos + 6:pos + 12]
        if not low.startswith('\\u') or not 0xDC00 <= int(low[2:], 16) <= 0xDFFF:
            raise ValueError('invalid JSON surrogate pair')
        return json.loads('"' + raw[pos:pos + 12] + '"'), 12
    if len(value) == 1 and 0xDC00 <= ord(value) <= 0xDFFF:
        raise ValueError('unpaired JSON surrogate')
    return value, size


class ReplyDraft:
    def __init__(self, redactor: Redactor, emit: Callable[[dict[str, Any]], None],
                 field: Callable[[str, Any], Any] | None = None) -> None:
        self.emit = emit
        # the runtime's mapping of one top-level property to FERB's own answer form, when its provider receives the
        # schema reshaped (ferb/runtimes/schema_shape.py); None: the stream is already in that form
        self.field = field
        self.redactor = redactor
        self.redaction = redactor.stream()
        self.lines = _LineGuard()
        self.raw = ''
        self.pos = 0
        self.state = 'opening'
        self.metadata: dict[str, Any] = {}
        self.key = ''
        self.visible = False
        self.closed = False
        self.channel = 'reply'

    def _emit_text(self, text: str, *, final: bool = False) -> None:
        safe = self.redaction.feed(text.encode('utf-8'), final=final).decode('utf-8')
        guarded = self.lines.feed(safe, final=final)
        if guarded:
            self.visible = True
            self.emit({'type': self.channel + '_delta', 'text': guarded})

    def _eligible(self) -> bool:
        expected = set(SUPERVISOR_SCHEMA['properties']) - {'text'}
        if set(self.metadata) != expected:
            return False
        field = self.field
        metadata = ({key: field(key, value) for key, value in self.metadata.items()} if field is not None
                    else self.metadata)
        if metadata.get('kind') == 'task':
            # Only the public text streams. Validate the complete preceding metadata
            # exactly as a final outcome; never publish nested plans or execute them.
            try:
                SupervisorOutcome.model_validate({**metadata, 'text': ''})
            except ValueError:
                return False
            self.channel = 'plan'
            return True
        return (metadata.get('kind') in ('reply', 'clarify')
                and all(value is None or value == [] for key, value in metadata.items() if key != 'kind'))

    def feed(self, fragment: str) -> None:
        if self.closed:
            return
        self.raw += fragment
        try:
            decoded = self._read()
            if not self.closed:
                self._emit_text(decoded)
        except (ValueError, TypeError):
            self.abort()

    def _read(self) -> str:
        decoder = json.JSONDecoder()
        decoded_text: list[str] = []
        while self.pos < len(self.raw):
            if self.state != 'text' and self.raw[self.pos].isspace():
                self.pos += 1
                continue
            char = self.raw[self.pos]
            if self.state == 'opening':
                if char != '{':
                    raise ValueError('expected object')
                self.state = 'key'
            elif self.state == 'key':
                try:
                    key, end = decoder.raw_decode(self.raw, self.pos)
                except json.JSONDecodeError:
                    return ''.join(decoded_text)
                if not isinstance(key, str) or key in self.metadata or key not in SUPERVISOR_SCHEMA['properties']:
                    raise ValueError('unexpected or repeated key')
                self.key, self.pos, self.state = key, end, 'colon'
                continue
            elif self.state == 'colon':
                if char != ':':
                    raise ValueError('expected colon')
                self.state = 'value'
            elif self.state == 'value':
                if self.key == 'text':
                    if char != '"' or not self._eligible():
                        self.abort()
                        return ''
                    self.state = 'text'
                else:
                    try:
                        value, end = decoder.raw_decode(self.raw, self.pos)
                    except json.JSONDecodeError:
                        return ''.join(decoded_text)
                    self.metadata[self.key] = value
                    self.pos, self.state = end, 'separator'
                    continue
            elif self.state == 'separator':
                if char != ',':
                    raise ValueError('expected next field')
                self.state = 'key'
            elif self.state == 'text':
                if char == '"':
                    self.state = 'closing'
                else:
                    decoded = _character(self.raw, self.pos)
                    if decoded is None:
                        return ''.join(decoded_text)
                    value, size = decoded
                    decoded_text.append(value)
                    self.pos += size
                    continue
            elif self.state == 'closing':
                if char != '}':
                    raise ValueError('text must be last')
                self.state = 'complete'
            else:
                raise ValueError('trailing content')
            self.pos += 1
        return ''.join(decoded_text)

    def finish(self, validated_final: str) -> None:
        if self.closed:
            return
        try:
            matches = self.state == 'complete' and json.loads(self.raw) == json.loads(validated_final)
        except ValueError:
            matches = False
        if not matches:
            self.abort()
            return
        self._emit_text('', final=True)
        if self.channel == 'plan':
            self.emit({'type': 'plan_end'})
        self.closed = True

    def restart(self) -> None:
        """The planner looked something up (ORCH-516): text so far belonged to a turn that called a tool, never to the
        answer. Nothing shown yet: the draft starts over on the next fragments; something shown: it is withdrawn."""
        if self.visible:
            self.abort()
            return
        self.raw, self.pos, self.state, self.metadata, self.key = '', 0, 'opening', {}, ''
        self.closed, self.channel = False, 'reply'
        self.redaction, self.lines = self.redactor.stream(), _LineGuard()

    def abort(self) -> None:
        if not self.closed and self.visible:
            self.emit({'type': self.channel + '_abort', 'reason': 'The draft was interrupted; wait for the final reply.'})
        self.closed = True
