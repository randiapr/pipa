# GitOps on a local Kubernetes cluster: Flux (via the Flux Operator, which also serves the Flux web UI)
# keeps the `pipa` namespace in step with gitops/ on this repo's GitHub branch. Invoked as
# `just gitops::up`.

flux_namespace := "flux-system"
flux_operator_chart := "oci://ghcr.io/controlplaneio-fluxcd/charts/flux-operator"
# the branch Flux follows; manifests must be pushed there before the cluster converges
gitops_branch := env("GITOPS_BRANCH", "main")
flux_ui_port := "9080"

# Re-running is safe: existing secrets are kept. The Git token comes from GITHUB_TOKEN or
# `gh auth token`; without one Flux reads the repo anonymously (public repos only).
# install Flux and point it at this repo (needs a local Kubernetes cluster as the current kubectl context)
up:
    #!/usr/bin/env bash
    set -euo pipefail

    # refuse a remote cluster: Flux would start applying this repo to it. Set GITOPS_KUBE_CONTEXT
    # to the current context's name to allow one on purpose.
    context=$(kubectl config current-context 2>/dev/null || true)
    if [ -z "$context" ]; then
        echo "no kubectl context: start a local Kubernetes cluster first" >&2
        exit 1
    fi
    server=$(kubectl config view --minify -o jsonpath='{.clusters[0].cluster.server}')
    case "$server" in
        # loopback (kind, k3d), the 192.168.* VM bridges (minikube, Lima-based tools) and Docker Desktop
        https://127.0.0.1:*|https://localhost:*|https://\[::1\]:*|https://192.168.*|https://kubernetes.docker.internal:*) ;;
        *)
            if [ "${GITOPS_KUBE_CONTEXT:-}" != "$context" ]; then
                echo "context '$context' ($server) is not a local cluster; set GITOPS_KUBE_CONTEXT=$context to use it anyway" >&2
                exit 1
            fi
            ;;
    esac

    helm upgrade --install flux-operator {{ flux_operator_chart }} \
        --namespace {{ flux_namespace }} --create-namespace --wait

    kubectl create namespace pipa --dry-run=client -o yaml | kubectl apply -f -

    # secrets are created once and never overwritten, so tokens and the admin login stay stable
    if ! kubectl -n pipa get secret pipa-secrets >/dev/null 2>&1; then
        admin_password=$(openssl rand -hex 16)
        kubectl -n pipa create secret generic pipa-secrets \
            --from-literal=jwt-secret="$(openssl rand -base64 48)" \
            --from-literal=admin-password="$admin_password" \
            --from-literal=rustfs-access-key=rustfsadmin \
            --from-literal=rustfs-secret-key="$(openssl rand -hex 16)"
        echo "pipa admin login: admin / $admin_password"
    fi

    token="${GITHUB_TOKEN:-$(gh auth token 2>/dev/null || true)}"
    manifest=$(sed "s#refs/heads/main#refs/heads/{{ gitops_branch }}#" {{ justfile_directory() }}/gitops/clusters/local/flux-instance.yaml)
    if [ -n "$token" ]; then
        kubectl -n {{ flux_namespace }} create secret generic flux-system \
            --from-literal=username=git --from-literal=password="$token" \
            --dry-run=client -o yaml | kubectl apply -f -
    else
        echo "no GITHUB_TOKEN / gh login: Flux will read the repo anonymously" >&2
        manifest=$(printf '%s\n' "$manifest" | sed '/pullSecret:/d')
    fi
    printf '%s\n' "$manifest" | kubectl apply -f -

    kubectl -n {{ flux_namespace }} wait fluxinstance/flux --for=condition=Ready --timeout=5m
    echo "Flux is up and follows {{ gitops_branch }}; watch it with \`just gitops::status\` or \`just gitops::ui\`"

# what Flux has reconciled and the state of the pipa pods
status:
    kubectl -n {{ flux_namespace }} get fluxinstance,gitrepository,kustomization
    kubectl -n pipa get pods

# the Flux web UI at http://localhost:9080, until interrupted
ui:
    kubectl -n {{ flux_namespace }} port-forward svc/flux-operator {{ flux_ui_port }}:9080

# the pipa dashboard at http://localhost:3000 and its API at :8080 (the UI has the API address compiled in), until interrupted
app:
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    kubectl -n pipa port-forward svc/backend 8080:8080 &
    kubectl -n pipa port-forward svc/ui 3000:80 &
    wait

# render the pipa manifests offline (and schema-check them when kubeconform is installed)
validate:
    #!/usr/bin/env bash
    set -euo pipefail
    for dir in apps/pipa clusters/local; do
        rendered=$(kubectl kustomize {{ justfile_directory() }}/gitops/$dir)
        if command -v kubeconform >/dev/null 2>&1; then
            printf '%s\n' "$rendered" | kubeconform -strict -summary
        fi
    done
    command -v kubeconform >/dev/null 2>&1 || echo "manifests render; install kubeconform to schema-check them"

# remove Flux, its CRDs and the pipa stack from the cluster (the pipa data goes with its namespace); the cluster itself is kept
down:
    #!/usr/bin/env bash
    set -euo pipefail
    # the FluxInstance first, so Flux stops re-creating what is deleted below
    kubectl -n {{ flux_namespace }} delete fluxinstance flux --ignore-not-found --wait
    kubectl delete namespace pipa --ignore-not-found
    helm uninstall flux-operator --namespace {{ flux_namespace }} || true
    kubectl delete namespace {{ flux_namespace }} --ignore-not-found
    # the chart keeps the operator's CRDs on uninstall
    kubectl get crd -o name | grep '\.fluxcd\.controlplane\.io$' | xargs kubectl delete --ignore-not-found || true
