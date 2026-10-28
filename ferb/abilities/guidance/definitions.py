"""Explicit installed guidance IDs; caller input never chooses a package or filename."""
from pydantic import BaseModel, ConfigDict

from ferb.abilities import guidance
from ferb.abilities.contracts import AbilityDefinition, ProcedureResource


class GuidanceInput(BaseModel):
    model_config = ConfigDict(extra='forbid')


# Descriptions are the discovery index, not compressed copies of the procedures.
_GUIDANCE = (
    ('task_planning', 'Construct tasks/plans, conditions, task controls, delegates and standing send scopes.'),
    ('memory_intake', 'Capture durable ordinary-chat facts/preferences, usernames, dated facts and explicit remember/forget.'),
    ('scheduling', 'Create/change schedules, watches, reminders, timers; resolve times and save time corrections.'),
    ('continuous_work', 'Plan and execute repeated loop rounds, durable probes and held resources.'),
    ('delegated_conversations', 'Plan approved outside conversations, scoped replies, handover and notification rules.'),
    ('background_interfaces', 'Operate/inspect browser and Mac interfaces without disturbing the owner; profiles, gestures, tabs and attribution.'),
    ('sealed_login', 'Use saved usernames/secrets and hidden login/code input without exposing secret values.'),
    ('media_capture', 'Record/listen to exact task targets and inspect captured content and gaps.'),
    ('delegated_processes', 'Start/help/monitor named agents and processes; shell lifecycle, provenance and escape requests.'),
    ('artifact_work', 'Edit exact file/document content, produce complete deliverables and inspect changes/uploads.'),
    ('source_research', 'Establish source/account/date coverage, current availability and exhaustive comparison evidence.'),
)

DEFINITIONS = tuple(AbilityDefinition(
    id=name, version='1', description=description,
    procedure_resource=ProcedureResource(guidance, name + '.md'), input_schema=GuidanceInput,
    supported_interactions=(), required_capabilities=(),
    unavailable_reason='Guidance only: read and apply with existing tools; no one-call execution runner.',
) for name, description in _GUIDANCE)
