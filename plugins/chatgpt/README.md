# OpenTally for ChatGPT

This plugin uses OpenTally’s Streamable HTTP MCP endpoint. It provides accounting tools and an embedded Trial Balance view with page controls.

1. Run the OpenTally accounting app and [configure the OAuth MCP connector](../../connectors/mcp/README.md#chatgpt-and-claude-web).
2. In ChatGPT Work, open **Plugins**, choose **Add custom MCP server**, and enter the public HTTPS endpoint ending in `/mcp`.
3. Choose OAuth and follow your identity provider’s sign-in flow. Use the read scope for reports. Grant the write scope to authorized operators when they need to post vouchers, import data, reconcile a bank or close a period.
4. Open the plugin in a conversation and ask for a Trial Balance for a specific period. The plugin returns both the accounting figures and a report view when the host supports MCP Apps.

The plugin entry uses the MCP server’s tool catalog, OAuth metadata and UI resource. Use the repository’s MCP test suite and your ChatGPT workspace to verify the attachment to your own OAuth server. For publication, the provider’s plugin directory review is a separate step. [ChatGPT custom MCP setup](https://developers.openai.com/api/docs/guides/custom-mcp-server) · [ChatGPT plugin authentication](https://developers.openai.com/plugins/build/auth) · [MCP app UI](https://developers.openai.com/plugins/build/chatgpt-ui) · [Plugin submission](https://developers.openai.com/plugins/deploy/submission)
