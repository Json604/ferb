"""Owns each episode card's intent label (L29): what kind of task it was, as the learning model reads it, and the list
labels are chosen from. Must not decide sameness itself (R2): the model labels; code checks the answer's shape and
types and stores it as the card's own `episode_intent` node (keyed by the card's node id, its only source the card),
so forgetting a card removes only its label. A label that reuses an intent copies its name and description (they
describe the kind of task, not that run), so forgetting the run that first wrote them leaves that text in the intent's
other labels and its planner line (accepted residual, 2026-10-21). The intent list is derived from the current cards'
labels. A card read again (a new node of the same task) keeps its task's label (`carry_forward`), and a superseded
card's label is closed. Labels are the learner's bookkeeping: search never recalls them (`Learning.search`)."""
import json
from typing import Any, Iterable

from ferb.memory.learning import SLOT_TYPES, Learning, LearningRefused, digest
from ferb.memory.strict import JSON_TEXT, pairs, to_map

INTENT_KIND = 'episode_intent'
_TYPES: dict[str, Any] = {'string': str, 'number': (int, float), 'boolean': bool, 'array': list, 'object': dict}
# An intent id's characters: letters only, so no redaction pattern (a card number, a code) matches one, in a label's
# body or as a procedure's key (Learning.put redacts both); only a secret the owner stored, spelled in the letters a-p,
# could still alter one (Redactor replaces a stored value wherever it appears).
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


def _current_cards(learning: Learning) -> dict[str, str]:
    """The current episode cards' node ids -> recorded_at, read without their bodies (`Learning.items`' rule)."""
    return {r[0]: r[1] for r in learning.db.conn.execute(
        "SELECT id, recorded_at FROM learning_nodes WHERE kind = 'episode' AND valid_to IS NULL "
        "AND state NOT IN ('retired', 'rolled_back')")}


def labels(learning: Learning) -> dict[str, dict[str, Any]]:
    """Current card node id -> its label body (current cards only: a superseded card's label is stale)."""
    current = _current_cards(learning)
    return {n['key']: n['body'] for n in learning.items(INTENT_KIND) if n['key'] in current}


def _count(out: dict[str, dict[str, Any]], body: dict[str, Any], recorded_at: str) -> None:
    """One more card labelled `body`, given oldest label first: the newest label names the intent (its name,
    description and slots), the rule the planner's procedure list follows too (`named`)."""
    entry = out.setdefault(body['intent_id'], {'id': body['intent_id'], 'runs': 0, 'last': ''})
    entry.update(name=body['name'], description=body['description'], slots=body['slots'])
    entry['runs'] += 1
    entry['last'] = max(entry['last'], recorded_at)


def _ordered(out: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
    return sorted(out.values(), key=lambda i: (i['last'], i['id']), reverse=True)


def intents(learning: Learning) -> list[dict[str, Any]]:
    """The intents the current cards carry, most recently used first (by their newest card): id, name, description,
    slots (from its newest current label), runs, last."""
    current = _current_cards(learning)
    out: dict[str, dict[str, Any]] = {}
    for node in learning.items(INTENT_KIND):
        if node['key'] in current:
            _count(out, node['body'], current[node['key']])
    return _ordered(out)


def named(learning: Learning, intent_ids: Iterable[str]) -> dict[str, dict[str, Any]]:
    """For each of these intents that a current card carries: {label, last, labels, origins}: its newest current label
    node (its name and description), when it was last used (its newest card, as `intents` orders them), and the ids
    and the union of the origins of all its current labels. A label that reuses an intent copies its text from the
    one whose run wrote it, citing only its own card, so what shows that text carries and cites them all (L29
    re-review N2). One grouped query, then one node read per intent, so a planner turn never reads every label."""
    wanted = sorted(set(intent_ids))
    if not wanted:
        return {}
    rows = learning.db.conn.execute(
        "SELECT json_extract(l.body_json, '$.intent_id'), max(l.recorded_at || ' ' || l.id), max(c.recorded_at), "
        "json_group_array(DISTINCT l.id), json_group_array(DISTINCT o.value) "
        "FROM learning_nodes l JOIN learning_nodes c ON c.id = l.key LEFT JOIN json_each(l.origins_json) o "
        "WHERE l.kind = ? AND l.valid_to IS NULL AND l.state NOT IN ('retired', 'rolled_back') "
        "AND c.kind = 'episode' AND c.valid_to IS NULL AND c.state NOT IN ('retired', 'rolled_back') "
        f"AND json_extract(l.body_json, '$.intent_id') IN ({','.join('?' * len(wanted))}) "
        "GROUP BY json_extract(l.body_json, '$.intent_id')", (INTENT_KIND, *wanted)).fetchall()
    out: dict[str, dict[str, Any]] = {}
    for iid, newest, last, ids, made in rows:
        label = learning.get(str(newest).rsplit(' ', 1)[1])
        if label is not None:
            out[str(iid)] = {'label': label, 'last': str(last), 'labels': sorted(json.loads(ids)),
                             'origins': {o for o in json.loads(made) if o}}
    return out


def counted(known: list[dict[str, Any]], body: dict[str, Any], card: dict[str, Any]) -> list[dict[str, Any]]:
    """`known` (as `intents` returns it) with one more card labelled `body` (the newest label, so it names its intent):
    what `intents` returns now, without reading every card and label again."""
    out = {i['id']: dict(i) for i in known}
    _count(out, body, card['recorded_at'])
    return _ordered(out)


def unlabelled(learning: Learning) -> list[dict[str, Any]]:
    """The current episode cards without a current label, oldest first. A card whose label the owner rolled back
    waits until it changes (a new card node): its old answer would only be stored again."""
    done = labels(learning).keys() | _rolled_back(learning)
    return [n for n in learning.items('episode') if n['id'] not in done]


def _rolled_back(learning: Learning) -> set[str]:
    """The cards whose label the owner rolled back."""
    return {r[0] for r in learning.db.conn.execute(
        "SELECT key FROM learning_nodes WHERE kind = ? AND state = 'rolled_back'", (INTENT_KIND,))}


def carry_forward(learning: Learning) -> list[dict[str, Any]]:
    """Close each label whose card was superseded; a re-read card (a new node of the same task) without a label of its
    own first gets that label unchanged, as its maker wrote it, without a model call, so a card rebuild never renames
    an intent. The carried labels."""
    current = _current_cards(learning)
    stale = [n for n in reversed(learning.items(INTENT_KIND)) if n['key'] not in current]     # newest first
    if not stale:
        return []
    own = {n['key'] for n in learning.items(INTENT_KIND) if n['key'] in current} | _rolled_back(learning)
    carried = []
    for old in stale:
        card = learning.get(old['key'])
        task_card = learning.current_card(str(card['key'])) if card and card['kind'] == 'episode' else None
        if task_card is not None and task_card['id'] not in own:
            try:
                carried.append(learning.put(INTENT_KIND, task_card['id'], old['body'], [task_card['id']],
                                            made_by=old['made_by'], state='active'))
                own.add(task_card['id'])
            except LearningRefused:
                pass                                     # e.g. forgotten: the card is labelled by the model instead
        learning.db.conn.execute('UPDATE learning_nodes SET valid_to=? WHERE id=? AND valid_to IS NULL',
                                 (learning.now(), old['id']))
    return carried


def typed(values: dict[str, Any], slots: dict[str, str]) -> None:
    """LearningRefused unless `values` name exactly `slots` and each value has its slot's JSON type (a label's values,
    and the planner's values for a procedure's slots: `Learning.slot_values`)."""
    if set(values) != set(slots):
        raise LearningRefused("the slot values must name exactly the intent's slots")
    for k, t in slots.items():
        if t not in _TYPES:                 # e.g. a type stored masked before slot types were restored (L29 O3)
            raise LearningRefused(f'slot {k} has no JSON type')
        v = values[k]
        if not isinstance(v, _TYPES[t]) or (t == 'number' and isinstance(v, bool)):
            raise LearningRefused(f'slot {k} is not a {t}')


def check(answer: dict[str, Any], offered: dict[str, dict[str, Any]]) -> dict[str, Any]:
    """The label body an answer stands for, or LearningRefused. An offered id reuses that intent; a new intent whose
    name is an existing intent's (case and spacing aside) is that intent."""
    values = to_map(answer['values'], json_text=True)
    chosen = str(answer.get('intent_id') or '')
    if not chosen:
        # model-written free text later shown to the planner: one line (L29 review M2)
        name, description = ' '.join(str(answer['name']).split()), ' '.join(str(answer['description']).split())
        if not name or not description:
            raise LearningRefused('a new intent needs a name and a description')
        chosen = intent_id(name)
        if chosen not in offered:
            slots = to_map(answer['slots'])
            if any(not k.strip() for k in slots) or any(t not in SLOT_TYPES for t in slots.values()):
                raise LearningRefused('slots must be named and declare JSON types')
            typed(values, slots)
            return {'intent_id': chosen, 'name': name, 'description': description, 'slots': slots, 'values': values}
    if chosen not in offered:
        raise LearningRefused('the intent id is not one of the offered intents')
    intent = offered[chosen]
    typed(values, intent['slots'])
    return {'intent_id': chosen, 'name': intent['name'], 'description': intent['description'],
            'slots': intent['slots'], 'values': values}


def label(learning: Learning, card: dict[str, Any], answer: dict[str, Any], offered: dict[str, dict[str, Any]],
          maker: str) -> dict[str, Any]:
    """Check a label answer for `card` against the known intents (`offered`, by id) and store it as the card's own
    label node."""
    body = check(answer, offered)
    return learning.put(INTENT_KIND, card['id'], body, [card['id']], made_by=maker, state='active')
