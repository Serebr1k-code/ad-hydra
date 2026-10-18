# Traffic workspace

In **Services**, click a service's **Color** button. Choose one of twelve named presets, enter a custom `#RRGGBB` color, or reset to the automatic palette. Save applies to all users and survives restarts. It does not reconnect clients or change relay configuration. The service dropdown and stream-card stripe use the same color. Unconfigured services use distinct presets until all twelve have been used; you can deliberately reuse a configured color.

In **Traffic**, the checkbox inside each stream card selects that stream for filter creation. Clicking the rest of the card opens its conversation. Selection remains limited to fifty streams from one service.

On desktop, drag the divider between cards and the conversation. You can also focus the divider and use the arrow keys. **Expand traffic** fills the browser window and preserves the current card-column width. **Exit expanded view** or Escape returns to the regular layout. Live delivery, the selected session and filter selections continue across expansion.

Below 768px window width, the list sits above the conversation. When the conversation pane itself is at least 768px wide, client blocks align left and server blocks align right; narrower panes use the full available width. Blocks retain their chronological order and direction labels in decoded, saved UTF-8 and saved hex views.

**Flags**, **Submissions**, **Submission attempts** and Farm **Run history** start collapsed. Click a heading to open its list, pagination and controls. Closing a history stops its background list polling. Starting a Farm run opens its history automatically.

Service colors are saved in Core's SQLite database and are included in database backups. The networking configuration export does not contain this presentation setting. The public color API is documented in Swagger: `PUT /api/v1/services/{id}/color`, with `{"color":"#66d9e8"}` or `{"color":null}` to reset. No internal Proxy protocol changes are required.
