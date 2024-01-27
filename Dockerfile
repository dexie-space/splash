FROM alpine:3.14

RUN apk add --no-cache curl jq

WORKDIR /app

ARG GITHUB_ORG
ARG GITHUB_REPO
ENV GITHUB_ORG $GITHUB_ORG
ENV GITHUB_REPO $GITHUB_REPO


RUN arch=$(uname -m) && \
    os=$(uname -s) && \
    case "$arch" in \
        x86_64) arch="amd64" ;; \
        aarch64) arch="arm64" ;; \
        *) echo "Unsupported architecture: $arch" && exit 1 ;; \
    esac && \
    case "$os" in \
        Linux) os="linux" ;; \
        Darwin) os="darwin" ;; \
        *) echo "Unsupported OS: $os" && exit 1 ;; \
    esac && \
    RELEASE_URL=$(curl -sfL https://api.github.com/repos/${GITHUB_ORG}/${GITHUB_REPO}/releases/latest | \
    jq -r --arg binary "${GITHUB_REPO}-${os}-${arch}" '.assets[] | select(.name == $binary) | .browser_download_url') && \
    echo "Downloading binary" && \
    curl -sfL ${RELEASE_URL} -o ${GITHUB_REPO} && \
    chmod +x ${GITHUB_REPO}

RUN cat > /entrypoint.sh <<'EOF'
#!/bin/sh
exec /app/${GITHUB_REPO} "$@"
EOF

RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
