# Ghostex terms

What the words we use for Ghostex mean, so users, agents and code reviews stay on the same page. Add or update an entry whenever the user defines a new term or renames a concept.

- **Orchestrator**: the agent session the user talks to about a stream of work. It plans the work, hands it to threads, keeps track of them and reports back. Formerly called Coordinator (renamed 2026-10-10); code, API routes and saved data still say `coordinator`, and `ghostex coordinator …` still works as a hidden alias of `ghostex orchestrator …`.
- **Thread**: an agent session an orchestrator starts and briefs to do one piece of work. It sits under its orchestrator in the sidebar, and its final message is its report to the orchestrator.
