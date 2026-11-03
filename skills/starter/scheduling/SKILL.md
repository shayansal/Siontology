---
name: scheduling
description: Find a time for a meeting and put it in the calendar once it is agreed. Use with the calendar tools (list_events, create_event) when asked to plan or book a meeting.
---

# Finding a time

1. Ask for, or work out, what is needed: how long, by when, who, and the
   hours that suit (in their time zone).
2. Read the calendar for that span with `list_events`, in UTC. Find gaps
   long enough, with a margin around other events, inside the hours that
   suit.
3. Offer two or three of them, written in the person's own time zone with
   the zone named, nearest first.
4. Add the event with `create_event` only once the person chose one:
   summary, start and end in UTC, the place or link, and a line of what it
   is for.
5. Say what you added, with its time in their zone. Never remove an event
   unless asked, and then only the one named.
