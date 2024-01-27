FROM alpine:3.14

RUN apk add --no-cache curl jq

WORKDIR /app
ENV APP_NAME=splash

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
    release_url=$(curl -sfL https://api.github.com/repos/dexie-space/splash/releases/latest | \
    jq -r --arg binary "${APP_NAME}-${os}-${arch}" '.assets[] | select(.name == $binary) | .browser_download_url') && \
    curl -sfL $release_url -o splash && \
    chmod +x $APP_NAME

ENTRYPOINT ["/app/splash"]
