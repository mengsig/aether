// The shell header of each route with no nav entry; a nav route is named by its nav label.

export const screenTitles: Readonly<Record<string, string>> = {
  "#/agents/:conn/:agent": "Agent",
  "#/controls/:conn/:agent": "Controls",
  "#/memory/:conn": "Memory",
  "#/archon/initiatives/:id": "Initiative",
  "#/archon/items/:item": "Item",
  "#/archon/items/:item/runs/:run": "Run",
  "#/github/:host/:owner/:repo/:number": "GitHub",
  "#/learn/sessions/:id": "Session",
  "#/pair/:daemon": "Pairing",
};
