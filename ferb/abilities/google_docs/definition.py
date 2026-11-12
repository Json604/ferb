"""Google Docs, Sheets and Slides (HAND-507): a procedure to read; the writing itself is the browser tools'."""
from pydantic import BaseModel, ConfigDict, Field

from ferb.abilities import google_docs
from ferb.abilities.contracts import AbilityDefinition, ProcedureResource


class GoogleDocsInput(BaseModel):
    model_config = ConfigDict(extra='forbid')
    tab: str = Field(description='Existing task-owned tab showing the Google Doc, Sheet or Slides file.')


DEFINITION = AbilityDefinition(
    id='google_docs',
    version='1',
    description='Google Docs, Sheets and Slides: how text gets into their canvas-drawn editors (paste with html at the '
                'caret, typing line by line, importing a file) and how to read it back (export addresses). No '
                'runner: read the procedure with abilities.read and follow it with the browser tools.',
    procedure_resource=ProcedureResource(google_docs),
    input_schema=GoogleDocsInput,
    supported_interactions=(),
    required_capabilities=('browser.snapshot', 'browser.screenshot', 'browser.point', 'browser.act', 'browser.open'),
    unavailable_reason='A procedure to follow with the browser tools; there is no one-call runner.',
)
