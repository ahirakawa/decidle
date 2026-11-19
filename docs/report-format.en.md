## How to report

When you finish work that needs a report, write the report to a JSON file. The human reads the page made from that JSON. Assume the reply in the terminal is not read, and keep it short.

1. Write the report with Write to `<working directory>/.decidle/<task-name>.report.json`. Name the task with letters, digits and hyphens. If you continue the same task, overwrite the same file.
2. Keep the reply in the terminal within 400 characters. Write only the conclusion and whether there is anything to decide.

The JSON has the shape below. Every key is required. Use `[]` for an array with nothing in it.

```json
{
  "title": "Name of the work",
  "goal": "What the human was trying to do with this work, in one sentence",
  "conclusion": "What happened, or what you changed. Within 200 characters. No breakdown or steps.",
  "decisions": [
    {
      "question": "What you want decided, in one sentence. Within 40 characters",
      "options": [
        { "label": "Name of a choice", "effect": "What happens if this is chosen" },
        { "label": "Another choice", "effect": "What happens if this is chosen" }
      ],
      "recommend": "The same string as the label of the choice you recommend",
      "reason": "Why you recommend it"
    }
  ],
  "verified": ["What you checked by actually running it"],
  "unverified": ["What you did not check, or could not check"],
  "deliverables": [
    { "path": "docs/plan.md", "summary": "What you wrote or changed, in one sentence" },
    { "path": "content/items/", "summary": "When many files are in the same folder, you may write the folder path and the count" },
    {
      "path": "src/auth/client.ts",
      "summary": "For a code file, make one item per file and write at least one place in changes",
      "changes": [
        { "line": 42, "before": "How it worked. For a new file, \"none (new file)\"", "after": "How it works now" }
      ]
    }
  ]
}
```

Follow these rules when you write it.

- Put every file you edited in this turn in deliverables.
- In goal, write what the human was trying to do with this work (the purpose), not what the work was.
- In decisions, write only what only the human can decide. Write what you decided yourself in conclusion or verified.
- question is shown on a Home card, so keep it within 40 characters. Put the background and the reasons in reason or in the effect of options.
- In verified, write only what you checked by actually running a command in this turn.
- When you use a name or number you gave to something during the work, say again what it means each time.
- Do not stop at the name of a field or a policy. Write what will actually be done and what will change.
