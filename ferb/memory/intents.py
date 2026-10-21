"""Owns each episode card's intent label (L29): what kind of task it was, as the learning model reads it, and the list
labels are chosen from. Must not decide sameness itself (R2): the model labels; code checks the answer's shape and
types and stores it as the card's own `episode_intent` node (keyed by the card's node id, its only source the card),
so forgetting a card removes only its label. The intent list is derived from the current cards' labels."""
from typing import Any

from ferb.memory.learning import Learning, LearningRefused, digest
from ferb.memory.strict import JSON_TEXT, pairs, to_map

INTENT_KIND = 'episode_intent'
SLOT_TYPES = ('string', 'number', 'boolean', 'array', 'object')
_TYPES: dict[str, Any] = {'string': str, 'number': (int, float), 'boolean': bool, 'array': list, 'object': dict}
# An intent id's characters: letters only, so no redaction rule (a card number, a code) can ever match one, in a
# label's body or as a procedure's key (Learning.put redacts both).
_LETTERS = str.maketrans('0123456789abcdef', 'abcdefghijklmnop')
INTENT_SCHEMA: dict[str, Any] = {
    'type': 'object', 'additionalProperties': False,
    'required': ['intent_id', 'name', 'description', 'slots', 'values'],
    'properties': {
        'intent_id': {'type': 'string', 'description': "an offered intent's id, or empty for a new intent"},
        'name': {'type': 'string', 'description': 'a new intent: a short name; empty when reusing one'},
        'description': {'type': 'string', 'description': 'a new intent: one line; empty when reusing one'},
        'slots': pairs({'type': 'string', 'description': "a new intent: the slot's JSON type"}),
        'values': pairs(JSON_TEXT)}}


def intent_id(name: str) -> str:
    """An intent's id: from its name, case and spacing aside, so the same name is always the same intent."""
    return 'i-' + digest(' '.join(name.casefold().split()))[:16].translate(_LETTERS)


def labels(learning: Learning) -> dict[str, dict[str, Any]]:
    """Current card node id -> its label body (current cards only: a superseded card's label is stale)."""
    current = {n['id'] for n in learning.items('episode')}
    return {n['key']: n['body'] for n in learning.items(INTENT_KIND) if n['key'] in current}


def intents(learning: Learning) -> list[dict[str, Any]]:
    """The intents the current cards carry, most recently used first (by their newest card): id, name, description,
    slots, runs, last."""
    current = {n['id']: n for n in learning.items('episode')}
    out: dict[str, dict[str, Any]] = {}
    for node in learning.items(INTENT_KIND):
        card = current.get(node['key'])
        if card is None:
            continue
        b = node['body']
        entry = out.setdefault(b['intent_id'], {'id': b['intent_id'], 'name': b['name'],
                                                'description': b['description'], 'slots': b['slots'],
                                                'runs': 0, 'last': ''})
        entry['runs'] += 1
        entry['last'] = max(entry['last'], card['recorded_at'])
    return sorted(out.values(), key=lambda i: (i['last'], i['id']), reverse=True)


def unlabelled(learning: Learning) -> list[dict[str, Any]]:
    """The current episode cards without a current label, oldest first."""
    done = labels(learning)
    return [n for n in learning.items('episode') if n['id'] not in done]


def _typed(values: dict[str, Any], slots: dict[str, str]) -> None:
    """LearningRefused unless `values` name exactly `slots` and each value has its slot's JSON type."""
    if set(values) != set(slots):
        raise LearningRefused("the slot values must name exactly the intent's slots")
    for k, t in slots.items():
        v = values[k]
        if not isinstance(v, _TYPES[t]) or (t == 'number' and isinstance(v, bool)):
            raise LearningRefused(f'slot {k} is not a {t}')


def check(answer: dict[str, Any], offered: dict[str, dict[str, Any]]) -> dict[str, Any]:
    """The label body an answer stands for, or LearningRefused. An offered id reuses that intent; a new intent whose
    name is an existing intent's (case and spacing aside) is that intent."""
    values = to_map(answer['values'], json_text=True)
    chosen = str(answer.get('intent_id') or '')
    if not chosen:
        name, description = str(answer['name']).strip(), str(answer['description']).strip()
        if not name or not description:
            raise LearningRefused('a new intent needs a name and a description')
        chosen = intent_id(name)
        if chosen not in offered:
            slots = to_map(answer['slots'])
            if any(not k.strip() for k in slots) or any(t not in SLOT_TYPES for t in slots.values()):
                raise LearningRefused('slots must be named and declare JSON types')
            _typed(values, slots)
            return {'intent_id': chosen, 'name': name, 'description': description, 'slots': slots, 'values': values}
    if chosen not in offered:
        raise LearningRefused('the intent id is not one of the offered intents')
    intent = offered[chosen]
    _typed(values, intent['slots'])
    return {'intent_id': chosen, 'name': intent['name'], 'description': intent['description'],
            'slots': intent['slots'], 'values': values}


def label(learning: Learning, card: dict[str, Any], answer: dict[str, Any], offered: dict[str, dict[str, Any]],
          maker: str) -> dict[str, Any]:
    """Check a label answer for `card` against the known intents (`offered`, by id) and store it as the card's own
    label node."""
    body = check(answer, offered)
    return learning.put(INTENT_KIND, card['id'], body, [card['id']], made_by=maker, state='active')
