# GitOps on a local Kubernetes cluster: Flux (via the Flux Operator, which also serves the Flux web UI)
# keeps the `pipa` namespace in step with gitops/ on this repo's GitHub branch. Invoked as
# `just gitops::up`.

flux_namespace := "flux-system"
flux_operator_chart := "oci://ghcr.io/controlplaneio-fluxcd/charts/flux-operator"
# the branch Flux follows; manifests must be pushed there before the cluster converges
gitops_branch := env("GITOPS_BRANCH", "main")
flux_ui_port := "9080"
# where the cluster's volumes and the generated secrets live (gitignored); the cluster's VM must
# share this directory with your machine (the home directory is by default)
gitops_data := justfile_directory() / "gitops-data"

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

    # no --version: the OCI chart resolves to its newest release, so every `up` also upgrades the
    # operator (and, through the FluxInstance's `2.x`, Flux itself) to the latest
    helm upgrade --install flux-operator {{ flux_operator_chart }} \
        --namespace {{ flux_namespace }} --create-namespace --wait

    kubectl create namespace pipa --dry-run=client -o yaml | kubectl apply -f -

    # The secrets are kept in gitops-data/ (mode 600, gitignored) rather than only in the cluster,
    # so `down` and `up` or a new cluster hand the same values to the data on the volumes: the
    # admin login and tokens stay valid. A first run here adopts the values of a cluster that already
    # has the secret, and makes the rest.
    secrets_file="{{ gitops_data }}/pipa-secrets.env"
    mkdir -p "{{ gitops_data }}"
    [ ! -f "$secrets_file" ] || . "$secrets_file"
    made=""
    for pair in JWT_SECRET:jwt-secret ADMIN_PASSWORD:admin-password RUSTFS_ACCESS_KEY:rustfs-access-key \
                RUSTFS_SECRET_KEY:rustfs-secret-key RPC_SECRET:rpc-secret; do
        var="${pair%%:*}"; key="${pair#*:}"
        [ -z "${!var:-}" ] || continue
        value=$(kubectl -n pipa get secret pipa-secrets -o "jsonpath={.data['$key']}" 2>/dev/null | base64 -d 2>/dev/null || true)
        if [ -z "$value" ]; then
            case "$var" in
                JWT_SECRET) value=$(openssl rand -base64 48) ;;
                ADMIN_PASSWORD) value=$(openssl rand -hex 16) ;;
                RUSTFS_ACCESS_KEY) value=rustfsadmin ;;
                RUSTFS_SECRET_KEY) value=$(openssl rand -hex 16) ;;
                RPC_SECRET) value=$(openssl rand -hex 32) ;;
            esac
            made="$made $var"
        fi
        printf -v "$var" '%s' "$value"
    done
    (
        umask 077
        printf "JWT_SECRET='%s'\nADMIN_PASSWORD='%s'\nRUSTFS_ACCESS_KEY='%s'\nRUSTFS_SECRET_KEY='%s'\nRPC_SECRET='%s'\n" \
            "$JWT_SECRET" "$ADMIN_PASSWORD" "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY" "$RPC_SECRET" > "$secrets_file"
    )
    kubectl -n pipa create secret generic pipa-secrets \
        --from-literal=jwt-secret="$JWT_SECRET" --from-literal=admin-password="$ADMIN_PASSWORD" \
        --from-literal=rustfs-access-key="$RUSTFS_ACCESS_KEY" --from-literal=rustfs-secret-key="$RUSTFS_SECRET_KEY" \
        --from-literal=rpc-secret="$RPC_SECRET" --dry-run=client -o yaml | kubectl apply -f -
    case "$made" in *ADMIN_PASSWORD*) echo "pipa admin login: admin / $ADMIN_PASSWORD (kept in $secrets_file)" ;; esac

    just --justfile "{{ justfile_directory() }}/justfile" gitops::_storage pipa

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

# the RustFS volumes as PersistentVolumes on directories under `dir`, one per pod and reserved for
# its claim `data-rustfs-N` in namespace `ns`: they outlive the claims (Retain), and one left behind
# by a deleted claim is freed here for the next one. The directories are writable by the RustFS user.
_storage ns="pipa" dir=gitops_data:
    #!/usr/bin/env bash
    set -euo pipefail
    for i in 0 1 2 3; do
        mkdir -p "{{ dir }}/rustfs-$i"
        chmod 777 "{{ dir }}/rustfs-$i"
        pv="{{ ns }}-rustfs-$i"
        kubectl apply -f - <<EOF
    apiVersion: v1
    kind: PersistentVolume
    metadata:
      name: $pv
      labels:
        app.kubernetes.io/part-of: pipa-gitops
    spec:
      capacity:
        storage: 5Gi
      accessModes: ["ReadWriteOnce"]
      persistentVolumeReclaimPolicy: Retain
      storageClassName: pipa-local
      claimRef:
        namespace: {{ ns }}
        name: data-rustfs-$i
      hostPath:
        path: {{ dir }}/rustfs-$i
        type: DirectoryOrCreate
    EOF
        if [ "$(kubectl get pv "$pv" -o jsonpath='{.status.phase}')" = Released ]; then
            kubectl patch pv "$pv" --type merge -p '{"spec":{"claimRef":{"uid":null,"resourceVersion":null}}}'
        fi
    done

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

# remove Flux and the pipa stack from the cluster; the data (gitops-data/), the Flux Operator, its CRDs and the cluster itself are kept
down:
    #!/usr/bin/env bash
    set -euo pipefail
    # the FluxInstance first, so Flux stops re-creating what is deleted below; this removes Flux
    # (its controllers and the sync) but not the operator, which `up` keeps on the newest release
    kubectl -n {{ flux_namespace }} delete fluxinstance flux --ignore-not-found --wait
    kubectl delete namespace pipa --ignore-not-found

# delete everything pipa keeps: the stack, its volumes and gitops-data/ with the data and the secrets (asks first)
[confirm("Delete the pipa stack, its volumes and gitops-data/ (all pipa data and the generated secrets)?")]
purge:
    #!/usr/bin/env bash
    set -euo pipefail
    just --justfile "{{ justfile_directory() }}/justfile" gitops::down
    kubectl delete pv -l app.kubernetes.io/part-of=pipa-gitops --ignore-not-found
    rm -rf "{{ gitops_data }}"
