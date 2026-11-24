# Miniflare (local Cloudflare R2) for the r2:// test tier. See server.mjs.
FROM node:24-slim@sha256:873f586f3794f52281bd039bf02c048a0be08fde619338f9941bad1eb6b34530

WORKDIR /app
COPY package.json package-lock.json ./
# workerd's postinstall only fetches its platform binary from the lockfile's
# optional dependencies; nothing else runs install scripts that matter here.
RUN npm ci --omit=dev --no-audit --no-fund && npm cache clean --force
COPY server.mjs ./

USER node
ENV R2_READY_FILE=/tmp/r2-ready
CMD ["node", "server.mjs"]
