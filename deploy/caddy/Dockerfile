# Caddy with ACME DNS-challenge providers baked in (stock caddy:2-alpine has none).
# Needed by the "LAN + HTTPS" network mode: certificates for a LAN-only host are
# obtained via DNS-01 (a TXT record at the provider), so no inbound port is required.
FROM caddy:2-builder-alpine AS builder

# The pinned modules below are transitive dependencies of the last Caddy release, raised past
# their published CVEs (trivy flags the built binary otherwise). Drop a pin once a Caddy
# release ships that version or newer.
#
# The module and build caches are mounts rather than layer content: xcaddy pulls a few hundred
# modules, and on a link that resets a download part-way (an IPv6 path can do that) a retry
# then resumes instead of starting the whole fetch over.
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    xcaddy build \
    --with github.com/caddy-dns/duckdns \
    --with github.com/caddy-dns/cloudflare \
    --replace golang.org/x/crypto=golang.org/x/crypto@v0.57.0 \
    --replace golang.org/x/net=golang.org/x/net@v0.62.0 \
    --replace golang.org/x/text=golang.org/x/text@v0.42.0 \
    --replace google.golang.org/grpc=google.golang.org/grpc@v1.84.0 \
    --replace github.com/go-jose/go-jose/v4=github.com/go-jose/go-jose/v4@v4.1.6

FROM caddy:2-alpine
# Alpine ships package fixes before the caddy image is rebuilt upstream: take them now.
RUN apk upgrade --no-cache
COPY --from=builder /usr/bin/caddy /usr/bin/caddy
