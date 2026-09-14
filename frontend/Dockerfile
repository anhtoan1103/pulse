# Standard Next.js standalone-output multi-stage build.
FROM node:20-slim AS deps
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci

FROM node:20-slim AS builder
WORKDIR /app
# NEXT_PUBLIC_* vars are inlined into the client bundle at build time, not
# read at container runtime — must be passed as a build arg, not just a
# docker-compose `environment:` entry (which only affects the running process).
ARG NEXT_PUBLIC_API_URL
ENV NEXT_PUBLIC_API_URL=$NEXT_PUBLIC_API_URL
COPY --from=deps /app/node_modules ./node_modules
COPY . .
RUN npm run build

FROM node:20-slim AS runtime
WORKDIR /app
ENV NODE_ENV=production
RUN useradd --create-home --shell /bin/false pulse
COPY --from=builder /app/public ./public
COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
USER pulse
EXPOSE 3000
CMD ["node", "server.js"]
