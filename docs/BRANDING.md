# Praxis identity

The editable vector master is `static/logo.svg`: a geometric P with a green
decision path and an ochre checkpoint. It contains two named paths, no raster
image, external resource or font. It can be edited directly in Inkscape.
Keep the enclosing `praxis-mark` group for share-card generation.

The homepage uses warm paper, deep green and serif headings. The dashboard uses
matching green/ochre accents in dark and light themes. Default avatars use the
SVG; uploaded personal avatars keep their existing behavior.

Regenerate all derivatives from the repository root (Node and Sharp required):

```bash
npm install --prefix /tmp/praxis-logo sharp
node scripts/build_logo.mjs static/logo.svg static /tmp/praxis-logo/node_modules/sharp
```

This synchronizes `homepage/assets/logo.svg`, dashboard and homepage PNG icons,
the six-size favicon ICO and the 1200×630 homepage share card. The PNG icons have
a paper background for compatibility. Do not edit derivative images separately.

For an installed dashboard, rebuild and run the updated binary, then update its
disk assets with `praxis repair-assets --directory /path/to/installation
--update-dashboard`. This command backs up changed dashboard assets and
preserves custom workflows and data. Refresh the browser after restarting.
Deploy the `homepage/` directory through the website's existing hosting setup.

`src/branding.rs` defines the HTTP app identity: **Praxis** and
**https://getpraxis.boo**. OpenRouter uses its dedicated app-attribution headers;
other HTTP providers receive the versioned User-Agent. Script plugins use the
same name and URL without a Rust version. This metadata is independent of
conversation names and model/persona instructions.
