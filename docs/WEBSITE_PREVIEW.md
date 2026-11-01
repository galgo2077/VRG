# VRG website preview

This folder contains a **static, bilingual** site design:

- `index.html` — English landing page.
- `es/index.html` — Spanish landing page.
- `site.css` — responsive styling, no CDN, fonts, external JavaScript, analytics or tracking.
- `public/` — public documentation.

## Local preview

Open `docs/index.html` in a browser, or serve this directory through any standard local static HTTP server. No build step is required.

## GitHub Pages deployment

The site is **not automatically live** just because these files are committed. This source includes a GitHub Pages Actions workflow. A repository admin must select **GitHub Actions** as the Pages publishing source in Settings → Pages if it is not already enabled, then verify the workflow and the resulting URL. Do not infer that a Pages URL is live merely from a successful commit.

Do not activate a Pages URL without first ensuring the website does not claim that unavailable source files or installers have been released.

## Design direction

- Midnight graphite background with teal/lavender accents.
- VRG typographic mark, not an official logo asset.
- Hero with conceptual knowledge graph, feature cards, architecture flow, privacy principles and roadmap CTA.
- Responsive layout, keyboard focus, skip link and reduced-motion support.

Replace the typographic mark only when the actual approved VRG logo asset is available. Do not fetch or guess personal design assets.

## Current source and setup

- [Phase 4 source release](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4)
- [English install and verification web page](setup.html)
- [Instalación en español](es/setup.html)
- [Detailed setup Markdown](public/SETUP.md)

The F4 tag ships source code, not a precompiled binary; the Cargo package reports 0.1.0. The public site must explain that distinction until distribution versioning is aligned.
