---
name: document-brief
description: Read documents and write a brief of what they say, with where each point comes from. Use with the documents tools (list_documents, read_document, search_documents) for a summary, a comparison or an answer from a set of files.
---

# Briefing from documents

1. Find the documents: `list_documents` (narrowed with `glob`) or
   `search_documents` for the words that matter. Read each with
   `read_document`, in parts (`offset`, `limit`) when it is long.
2. Note each point with where it is: the file, and the heading or line.
3. Write the brief: the answer or the gist first, in a few sentences;
   then the points that support it, each with its place; then what the
   documents disagree on, and what they do not say.
4. Quote numbers and dates exactly as written. Do not fill a gap with a
   guess: say it is not there.
5. If asked to keep the brief, write it with `write_document` as Markdown,
   next to the documents or where the person says.
