# Miniflare (local Cloudflare R2) for the r2:// test tier. See server.mjs.
FROM node:25-slim@sha256:81db02c4b671288a03915da9534dbd54f96d0e7c24d80ccc54f5b36b2e684370

WORKDIR /app
COPY package.json package-lock.json ./
# workerd's postinstall only fetches its platform binary from the lockfile's
# optional dependencies; nothing else runs install scripts that matter here.
RUN npm ci --omit=dev --no-audit --no-fund && npm cache clean --force
COPY server.mjs ./

USER node
ENV R2_READY_FILE=/tmp/r2-ready
CMD ["node", "server.mjs"]
