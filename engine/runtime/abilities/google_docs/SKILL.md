# Google Docs, Sheets and Slides

Shipped by FERB; a procedure to follow with the browser tools, not a runner. What is verified and what isn't is said
at each step: the signed-in behaviour of Google's editors is unverified until the owner's live checks (LIVE-916, 917).

## What the page is

Docs and Slides draw the document on a canvas; the snapshot's text and elements show the menus and some words, not
the document. Keys go to a small hidden field: Docs' "Document content" textbox (in a frame under the canvas), Sheets'
cell input (kept outside the page). FERB types and presses keys into such a field (it is focused by script, never
clicked); a click on the drawn page itself goes through a screenshot point.

## Writing, in this order

1. **Paste at the caret, with html.** Put the caret where the text goes (a click on that place through a screenshot
   point, or keys: cmd+Home for the start, cmd+End for the end), then `browser_act` action `paste` on the document's
   text field with `text` (the plain text) and `html` (h1-h3, p, ul/ol/li, b, i, a) so headings and lists stay
   headings and lists. If the result says `paste_not_taken`, go to 2.
2. **Type line by line.** `browser_act` action `type` on the same field: it goes at the caret (insert) and each line
   break is pressed as Enter, each tab as Tab. In Sheets, select the first cell first; "a\tb\nc\td" fills a 2x2 block
   (Tab moves right, Enter moves down). Formatting: the editor's own shortcuts (cmd+b, cmd+alt+1..3 for headings in
   Docs) or its menus.
3. **Import a file.** Write the content to a file in your workspace (.md or .docx for Docs, .csv or .xlsx for
   Sheets), then use the editor's own import (File → Import in Sheets, File → Open → Upload in Docs) and
   `browser_act` action `upload` onto the file field or drop zone it shows. This worked live for a CSV into Sheets.

Never use the system clipboard (cmd+v, or the shell's pbcopy) unless the owner asked: it is the owner's, and it holds
whatever they last copied.

## Reading back before saying done

- Docs: `browser_open` `https://docs.google.com/document/d/<id>/export?format=txt` (or `format=md`); the file
  arrives as a download (`downloads` in the result, or `browser_downloads`); read it as a file.
- Sheets: `https://docs.google.com/spreadsheets/d/<id>/export?format=csv&gid=<gid>` (the gid is in the sheet's
  address after `#gid=`).
- Slides: `https://docs.google.com/presentation/d/<id>/export?format=txt` (unverified).

Compare what you read with what you meant to write; say what differs. A fresh snapshot is no proof here: it doesn't
show the document's text.
