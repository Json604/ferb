"""Own typed observed recipient metadata. Must not choose a contact or infer identity from a name."""

from pydantic import BaseModel, Field


class Contact(BaseModel):
    """Observed recipient metadata. Resolve ambiguity from context or ask before choosing a person."""
    name: str = Field(description="the person as the owner named them, e.g. Rahul")
    selected: str = Field(description="the contact's name exactly as the app / page shows it")
    detail: str = Field(description="a number, username or subtitle the app / page shows for that contact")
