---
layout: page
title: Self-Host Kyomi
description: Run Kyomi on your own infrastructure. Full control, your LLM API key, no data leaves your network.
---

<div class="self-hosting-page">

<div style="text-align: center; padding-top: 3rem; margin-bottom: 2rem;">
  <h1 style="font-size: 2.5rem; font-weight: 700; margin-bottom: 0.5rem;">Run Kyomi Anywhere</h1>
  <p style="font-size: 1.25rem; color: var(--color-muted-foreground); max-width: 40rem; margin: 0 auto;">The full knowledge layer on your infrastructure. Open source (AGPL). All features included. Bring your own LLM key.</p>
</div>

<!-- Why Self-Host -->
<div style="max-width: 48rem; margin: 0 auto 3rem; display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1.5rem;">
  <div style="text-align: center; padding: 1.5rem;">
    <div style="font-size: 1.5rem; margin-bottom: 0.5rem;">
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--color-primary)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="display: inline-block;"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"></rect><path d="M7 11V7a5 5 0 0 1 10 0v4"></path></svg>
    </div>
    <strong>Full Control</strong>
    <p style="font-size: 0.875rem; color: var(--color-muted-foreground); margin-top: 0.25rem;">Data, queries, knowledge, and AI conversations stay on your infrastructure.</p>
  </div>
  <div style="text-align: center; padding: 1.5rem;">
    <div style="font-size: 1.5rem; margin-bottom: 0.5rem;">
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--color-primary)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="display: inline-block;"><path d="M12 2v20M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6"></path></svg>
    </div>
    <strong>Bring Your Own LLM</strong>
    <p style="font-size: 0.875rem; color: var(--color-muted-foreground); margin-top: 0.25rem;">Use your Anthropic, OpenAI, Gemini, or any OpenAI-compatible API key. Pay your provider directly.</p>
  </div>
  <div style="text-align: center; padding: 1.5rem;">
    <div style="font-size: 1.5rem; margin-bottom: 0.5rem;">
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="var(--color-primary)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="display: inline-block;"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="12" y1="18" x2="12" y2="12"></line><line x1="9" y1="15" x2="15" y2="15"></line></svg>
    </div>
    <strong>Open Source</strong>
    <p style="font-size: 0.875rem; color: var(--color-muted-foreground); margin-top: 0.25rem;">AGPL-3.0 licensed. Audit the code, contribute, or fork it. <a href="https://github.com/kyomi-ai/kyomi" style="color: var(--color-primary);">View on GitHub</a></p>
  </div>
</div>

<!-- Quick Start -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0 0 0.5rem;">Get Started in 30 Seconds</h2>
  <p style="color: var(--color-muted-foreground); margin-bottom: 1.5rem;">Choose your preferred installation method.</p>

  <h3 style="font-size: 1.1rem; margin-bottom: 0.75rem;">Option 1: Docker (recommended)</h3>
  <p style="font-size: 0.9rem; color: var(--color-muted-foreground); margin-bottom: 0.75rem;">One command installs Kyomi with PostgreSQL. Requires Docker with Compose.</p>

  <div style="background: #1a1a2e; color: #e0e0e0; padding: 1.25rem; border-radius: 0.5rem; font-family: monospace; font-size: 0.875rem; overflow-x: auto; margin-bottom: 1.5rem;">
    <span style="color: #6b7280;">$</span> curl -fsSL https://get.kyomi.ai | sh
  </div>

  <p style="font-size: 0.85rem; color: var(--color-muted-foreground); margin-bottom: 2rem;">The installer will prompt for your LLM API key and access URL, generate security keys, and start everything.</p>

  <h3 style="font-size: 1.1rem; margin-bottom: 0.75rem;">Option 2: Desktop (Standalone Binary)</h3>
  <p style="font-size: 0.9rem; color: var(--color-muted-foreground); margin-bottom: 0.75rem;">A single self-contained binary with the frontend, AI model, and server built in. Uses SQLite — no external database needed.</p>

  <div style="background: #1a1a2e; color: #e0e0e0; padding: 1.25rem; border-radius: 0.5rem; font-family: monospace; font-size: 0.875rem; overflow-x: auto; line-height: 1.8; margin-bottom: 1rem;">
    <span style="color: #6b7280;"># Download for your platform</span><br/>
    <span style="color: #6b7280;">$</span> curl -L https://github.com/kyomi-ai/kyomi/releases/latest/download/kyomi-linux-amd64.tar.gz | tar xz<br/>
    <br/>
    <span style="color: #6b7280;"># Set your LLM API key and run</span><br/>
    <span style="color: #6b7280;">$</span> export LLM_PROVIDER=anthropic<br/>
    <span style="color: #6b7280;">$</span> export LLM_API_KEY=sk-ant-...<br/>
    <span style="color: #6b7280;">$</span> ./kyomi
  </div>

  <p style="font-size: 0.85rem; color: var(--color-muted-foreground);">Open <code>http://localhost:3000</code> in your browser. Data is stored in <code>./data/</code> by default.</p>
</div>

<!-- Downloads -->
<div style="margin: 3rem 0;" id="downloads">
  <h2 style="text-align: center; margin-bottom: 0.5rem;">Downloads</h2>
  <p style="text-align: center; color: var(--color-muted-foreground); margin-bottom: 2rem;">Pre-built binaries for every major platform. All releases available on <a href="https://github.com/kyomi-ai/kyomi/releases" style="color: var(--color-primary);">GitHub Releases</a>.</p>

  <div style="max-width: 40rem; margin: 0 auto;">
    <table style="width: 100%; border-collapse: collapse; background: var(--color-background); border-radius: 0.5rem; overflow: hidden;">
      <thead>
        <tr style="background: var(--color-primary); color: white;">
          <th style="padding: 0.75rem 1rem; text-align: left;">Platform</th>
          <th style="padding: 0.75rem 1rem; text-align: left;">Architecture</th>
          <th style="padding: 0.75rem 1rem; text-align: center;">Download</th>
        </tr>
      </thead>
      <tbody>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Linux</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">x86_64 (amd64)</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;"><a href="https://github.com/kyomi-ai/kyomi/releases/latest/download/kyomi-linux-amd64.tar.gz" style="color: var(--color-primary); font-weight: 600;">tar.gz</a></td>
        </tr>
        <tr style="background: var(--color-muted);">
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Linux</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">ARM64 (aarch64)</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;"><a href="https://github.com/kyomi-ai/kyomi/releases/latest/download/kyomi-linux-arm64.tar.gz" style="color: var(--color-primary); font-weight: 600;">tar.gz</a></td>
        </tr>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">macOS</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Apple Silicon (arm64)</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;"><a href="https://github.com/kyomi-ai/kyomi/releases/latest/download/kyomi-macos-arm64.tar.gz" style="color: var(--color-primary); font-weight: 600;">tar.gz</a></td>
        </tr>
        <tr style="background: var(--color-muted);">
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">macOS</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Intel (amd64)</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;"><a href="https://github.com/kyomi-ai/kyomi/releases/latest/download/kyomi-macos-amd64.tar.gz" style="color: var(--color-primary); font-weight: 600;">tar.gz</a></td>
        </tr>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Docker</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Multi-arch (amd64 + arm64)</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;"><code style="font-size: 0.8rem;">ghcr.io/kyomi-ai/kyomi</code></td>
        </tr>
      </tbody>
    </table>
  </div>
</div>

<!-- System Requirements -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0 0 1.5rem;">System Requirements</h2>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 1.5rem;">
    <div>
      <h3 style="font-size: 1rem; margin-bottom: 0.5rem;">Desktop (Standalone Binary)</h3>
      <ul style="margin: 0; padding-left: 1.25rem; color: var(--color-muted-foreground); font-size: 0.9rem; line-height: 1.8;">
        <li>2 GB RAM minimum</li>
        <li>1 GB disk space</li>
        <li>An LLM API key</li>
        <li>Linux (glibc) or macOS</li>
        <li>No external database needed (uses SQLite)</li>
      </ul>
    </div>
    <div>
      <h3 style="font-size: 1rem; margin-bottom: 0.5rem;">Docker Compose</h3>
      <ul style="margin: 0; padding-left: 1.25rem; color: var(--color-muted-foreground); font-size: 0.9rem; line-height: 1.8;">
        <li>4 GB RAM minimum</li>
        <li>Docker with Compose plugin</li>
        <li>An LLM API key</li>
        <li>Uses PostgreSQL (included in compose)</li>
        <li>Better for production and multi-user</li>
      </ul>
    </div>
  </div>
</div>

<!-- Self-Hosted vs Cloud -->
<div style="margin: 3rem 0;">
  <h2 style="text-align: center; margin-bottom: 2rem;">Self-Hosted vs Cloud</h2>

  <div style="overflow-x: auto;">
    <table style="width: 100%; border-collapse: collapse; background: var(--color-background); border-radius: 0.5rem; overflow: hidden;">
      <thead>
        <tr style="background: var(--color-primary); color: white;">
          <th style="padding: 1rem; text-align: left;">Feature</th>
          <th style="padding: 1rem; text-align: center;">Self-Hosted</th>
          <th style="padding: 1rem; text-align: center;">Cloud</th>
        </tr>
      </thead>
      <tbody>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>AI Chat & Analysis</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Your LLM key, unlimited</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Included (usage-based)</td>
        </tr>
        <tr style="background: var(--color-muted);">
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Dashboards</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Unlimited</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Unlimited</td>
        </tr>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Datasources</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">All 9 supported</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">All 9 supported</td>
        </tr>
        <tr style="background: var(--color-muted);">
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>SQL Editor</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Full</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Full</td>
        </tr>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Forecasting</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Built-in</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Built-in</td>
        </tr>
        <tr style="background: var(--color-muted);">
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Kyomi Watch</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Included</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Included</td>
        </tr>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>MCP Support</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Included</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Included</td>
        </tr>
        <tr style="background: var(--color-muted);">
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Users</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Unlimited</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Per user (~$5/mo)</td>
        </tr>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Data Residency</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Your infrastructure</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Kyomi Cloud (AU)</td>
        </tr>
        <tr style="background: var(--color-muted);">
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Updates</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Manual (upgrade script)</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Automatic</td>
        </tr>
        <tr>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Support</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Community (GitHub)</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Email / Priority</td>
        </tr>
        <tr style="background: var(--color-muted);">
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);"><strong>Cost</strong></td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;">Free (+ your LLM costs)</td>
          <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border); text-align: center;"><a href="/pricing" style="color: var(--color-primary);">~$5/user/mo + AI</a></td>
        </tr>
      </tbody>
    </table>
  </div>
</div>

<!-- LLM Providers -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0 0 1rem;">Supported LLM Providers</h2>
  <p style="color: var(--color-muted-foreground); margin-bottom: 1.5rem;">Self-hosted Kyomi works with any of these AI providers. You bring your own API key.</p>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem;">
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong>Anthropic</strong>
      <p style="margin: 0.25rem 0 0; font-size: 0.85rem; color: var(--color-muted-foreground);">Claude 4, Claude 3.5 Sonnet</p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong>OpenAI</strong>
      <p style="margin: 0.25rem 0 0; font-size: 0.85rem; color: var(--color-muted-foreground);">GPT-4o, GPT-4 Turbo</p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong>Google Gemini</strong>
      <p style="margin: 0.25rem 0 0; font-size: 0.85rem; color: var(--color-muted-foreground);">Gemini 2.5 Pro, Flash</p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong>OpenAI-Compatible</strong>
      <p style="margin: 0.25rem 0 0; font-size: 0.85rem; color: var(--color-muted-foreground);">Ollama, vLLM, LiteLLM, any compatible API</p>
    </div>
  </div>
</div>

<!-- Documentation -->
<div style="margin: 3rem 0; padding: 2rem; border: 1px solid var(--color-border); border-radius: 0.75rem;">
  <h3 style="text-align: center; margin-bottom: 1.5rem;">Documentation</h3>
  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; font-size: 0.9rem;">
    <a href="https://github.com/kyomi-ai/kyomi/tree/main/docs/self-hosting" style="color: var(--color-primary); text-decoration: none; padding: 0.75rem; background: var(--color-muted); border-radius: 0.5rem; text-align: center; font-weight: 600;">Setup Guides</a>
    <a href="https://github.com/kyomi-ai/kyomi/tree/main/docs/self-hosting/configuration.md" style="color: var(--color-primary); text-decoration: none; padding: 0.75rem; background: var(--color-muted); border-radius: 0.5rem; text-align: center; font-weight: 600;">Configuration Reference</a>
    <a href="https://github.com/kyomi-ai/kyomi/tree/main/docs/self-hosting/troubleshooting.md" style="color: var(--color-primary); text-decoration: none; padding: 0.75rem; background: var(--color-muted); border-radius: 0.5rem; text-align: center; font-weight: 600;">Troubleshooting</a>
    <a href="https://github.com/kyomi-ai/kyomi/tree/main/docs/self-hosting/upgrading.md" style="color: var(--color-primary); text-decoration: none; padding: 0.75rem; background: var(--color-muted); border-radius: 0.5rem; text-align: center; font-weight: 600;">Upgrading</a>
  </div>
</div>

<!-- CTA -->
<div class="section" style="background: linear-gradient(135deg, #d97706 0%, #b45309 100%); color: white; border-radius: 1rem; text-align: center; padding: 4rem 1.5rem; margin: 4rem 0;">
  <h2 style="font-size: 2.25rem; font-weight: 700; margin-bottom: 0.75rem; color: white;">The knowledge layer, on your terms.</h2>
  <p style="font-size: 1.25rem; margin-bottom: 2rem; opacity: 0.95;">Open source. One command to install. All features included.</p>
  <div style="display: flex; justify-content: center; gap: 1rem; flex-wrap: wrap; margin-top: 2rem;">
    <a href="https://github.com/kyomi-ai/kyomi/releases" style="display: inline-flex; align-items: center; justify-content: center; background: white; color: #d97706; font-weight: 700; font-size: 1.125rem; padding: 1rem 2.5rem; border-radius: 0.5rem; text-decoration: none; transition: background-color 0.2s;">
      Download Binary
    </a>
    <a href="https://app.kyomi.ai/login" style="display: inline-flex; align-items: center; justify-content: center; background: transparent; color: white; font-weight: 700; font-size: 1.125rem; padding: 1rem 2.5rem; border-radius: 0.5rem; text-decoration: none; border: 2px solid white; transition: background-color 0.2s;">
      Or Try Cloud Free
    </a>
  </div>
</div>

</div>

<style scoped>
.self-hosting-page {
  max-width: 68rem;
  margin: 0 auto;
  padding: 0 1.5rem 4rem;
}

.self-hosting-page h2 {
  margin-top: 0;
}

table {
  font-size: 0.875rem;
}

@media (max-width: 768px) {
  table {
    font-size: 0.75rem;
  }

  th, td {
    padding: 0.5rem !important;
  }
}
</style>
