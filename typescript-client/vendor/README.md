# Pinned SDK package

`heliuslabs-zolana-0.1.7-alpha.tgz` is an unpublished build of `@heliuslabs/zolana` from [PR #317](https://github.com/helius-labs/zolana/pull/317).

- Source: `https://github.com/helius-labs/zolana`
- Revision: `e000afccd2506fe3e985663f341c601e1e77d0fb`
- Base: `e5e3fe8521dc0bf39ac77b60fae38f4aba5418ef`, the published `0.1.6-alpha` source.
- Package integrity is recorded in `../pnpm-lock.yaml`; the archive includes `dist/LICENSE`.

To rebuild with Node.js 24+, use a clean checkout of the pinned revision:

```bash
npm ci
npm run build:ts
npm pack --workspace @heliuslabs/zolana --ignore-scripts
```

Copy the resulting archive into this directory, run `pnpm install` in `typescript-client`, and review the archive and lockfile changes together.
