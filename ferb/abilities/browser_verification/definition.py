"""Browser verification discovery; execution stays unavailable until its workflow and input lifecycle land."""
from pydantic import BaseModel, ConfigDict, Field

from ferb.abilities import browser_verification
from ferb.abilities.contracts import AbilityDefinition, ProcedureResource


class BrowserVerificationInput(BaseModel):
    model_config = ConfigDict(extra='forbid')
    tab: str = Field(description='Existing task-owned tab in the authorized browser identity and profile.')


DEFINITION = AbilityDefinition(
    id='browser_verification',
    version='1',
    description='Browser verification: observe a visible check, use fresh evidence for its interaction, '
                'verify the website result, or preserve a precise owner handoff. Execution is not installed yet.',
    procedure_resource=ProcedureResource(browser_verification),
    input_schema=BrowserVerificationInput,
    supported_interactions=(),
    required_capabilities=('browser.snapshot', 'browser.screenshot', 'browser.point', 'browser.act', 'browser.wait'),
    unavailable_reason='The browser verification workflow and held-input lifecycle are not installed; '
                       'the procedure is available to read, but ability execution is unavailable.',
)
