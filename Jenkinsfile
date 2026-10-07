// NusaMart CI/DevSecOps pipeline
// Lint + test (memory & Postgres) & SCA/secret scan → DAST (ZAP) + load test (k6) pada server sementara di pod
// → build image (Kaniko) → scan image (Trivy) → update manifest k8s/ → ArgoCD sync.
//
// Kebutuhan di namespace Jenkins (dibuat oleh deploy/master-demo.ps1):
//   - secret docker-registry `ghcr-creds`  : push image ke GHCR
//   - credential Jenkins `github-creds`     : push commit manifest ke GitHub

pipeline {
  agent {
    kubernetes {
      yaml '''
apiVersion: v1
kind: Pod
spec:
  # Build Rust butuh RAM besar: jalankan di app-pool agar tidak berebut dengan ArgoCD/Jenkins/Prometheus.
  nodeSelector:
    kubernetes.civo.com/civo-node-pool: app-pool
  # Request sengaja kecil (hanya untuk penjadwalan): langkah berat (cargo, kaniko, ZAP) berjalan
  # bergantian, bukan bersamaan, jadi limit tinggi cukup tanpa memesan RAM node secara penuh.
  containers:
    - name: jnlp
      resources:
        requests: { cpu: 50m, memory: 128Mi }
        limits: { cpu: 500m, memory: 512Mi }
    - name: rust
      image: rust:1.98-slim-trixie
      resources:
        requests: { cpu: 200m, memory: 384Mi }
        limits: { cpu: "2", memory: 3Gi }
      command: ["sleep"]
      args: ["infinity"]
    - name: kaniko
      image: gcr.io/kaniko-project/executor:v1.24.0-debug
      resources:
        requests: { cpu: 200m, memory: 384Mi }
        limits: { cpu: "2", memory: 3Gi }
      command: ["sleep"]
      args: ["infinity"]
      volumeMounts:
        - name: docker-config
          mountPath: /kaniko/.docker
    - name: postgres
      image: postgres:17-alpine
      resources:
        requests: { cpu: 25m, memory: 32Mi }
        limits: { cpu: "1", memory: 512Mi }
      env:
        # Database sementara khusus CI (dibuang bersama pod), jadi trust auth cukup.
        - name: POSTGRES_HOST_AUTH_METHOD
          value: trust
    - name: zap
      image: ghcr.io/zaproxy/zaproxy:stable
      resources:
        requests: { cpu: 50m, memory: 192Mi }
        limits: { cpu: "1", memory: 1536Mi }
      command: ["sleep"]
      args: ["infinity"]
      volumeMounts:
        - name: zap-wrk
          mountPath: /zap/wrk
    - name: k6
      image: grafana/k6:2.3.0
      # Image k6 berjalan sebagai uid 12345, sedangkan workspace Jenkins milik uid 1000;
      # tanpa ini step `sh` gagal dengan "process apparently never started".
      securityContext:
        runAsUser: 1000
      env:
        - name: K6_NO_USAGE_REPORT
          value: "true"
      resources:
        requests: { cpu: 25m, memory: 32Mi }
        limits: { cpu: "1", memory: 512Mi }
      command: ["sleep"]
      args: ["infinity"]
    - name: trivy
      image: aquasec/trivy:0.74.0
      resources:
        requests: { cpu: 25m, memory: 64Mi }
        limits: { cpu: "1", memory: 1Gi }
      command: ["sleep"]
      args: ["infinity"]
      env:
        - name: DOCKER_CONFIG
          value: /kaniko/.docker
      volumeMounts:
        - name: docker-config
          mountPath: /kaniko/.docker
  volumes:
    - name: zap-wrk
      emptyDir: {}
    - name: docker-config
      secret:
        secretName: ghcr-creds
        items:
          - key: .dockerconfigjson
            path: config.json
'''
    }
  }

  options {
    timeout(time: 30, unit: 'MINUTES')
    disableConcurrentBuilds()
  }

  environment {
    IMAGE      = 'ghcr.io/dev-cer/ecommerce-app'
    GIT_REPO   = 'github.com/dev-cer/ecommerce-app.git'
    MANIFEST   = 'k8s/deployment.yaml'
    // Semua container di pod berbagi network, jadi ZAP & k6 bisa menembak server sementara via localhost.
    TEST_URL   = 'http://localhost:18080'
    // Postgres sidecar: test integrasi membuat database sendiri per test, server sementara memakai `postgres`.
    PG_URL     = 'postgres://postgres@localhost:5432/postgres'
  }

  stages {
    stage('Prepare') {
      steps {
        script {
          // Commit manifest dari Jenkins sendiri tidak perlu dibangun ulang (mencegah loop), tetapi
          // build yang ditekan manual ("Build Now") selalu jalan, juga saat HEAD adalah commit manifest.
          // Dicocokkan ke author commit + akhir baris judul saja: teks "[skip ci]" di tengah judul/isi
          // (mis. judul merge PR yang membahasnya) tidak boleh ikut melewati build.
          def author = sh(script: 'git log -1 --pretty=%ae', returnStdout: true).trim()
          def subject = sh(script: 'git log -1 --pretty=%s', returnStdout: true).trim()
          def fromJenkins = author == 'jenkins@kcd-bandung.local' || subject.endsWith('[skip ci]')
          def manual = !currentBuild.getBuildCauses('hudson.model.Cause$UserIdCause').isEmpty()
          env.SKIP_CI = (fromJenkins && !manual) ? 'true' : 'false'
          env.TAG = "${env.BUILD_NUMBER}-${env.GIT_COMMIT.substring(0, 7)}"
          currentBuild.displayName = "#${env.BUILD_NUMBER} · ${env.TAG}"
          if (env.SKIP_CI == 'true') {
            currentBuild.description = 'Commit manifest dari CI, dilewati'
            currentBuild.result = 'NOT_BUILT'
          }
        }
      }
    }

    stage('Quality & Security Gate') {
      when { expression { env.SKIP_CI != 'true' } }
      parallel {
        stage('Lint & Unit/Security Tests') {
          steps {
            container('rust') {
              sh '''
                mkdir -p reports && chmod 777 reports
                rustup component add rustfmt clippy
                cargo fmt --check
                cargo clippy --all-targets --locked -- -D warnings
                for i in $(seq 1 60); do
                  bash -c 'echo > /dev/tcp/127.0.0.1/5432' 2>/dev/null && break
                  sleep 1
                done
                TEST_DATABASE_URL="$PG_URL" cargo test --locked
                cargo build --locked
              '''
            }
          }
        }
        stage('SCA & Secret Scan') {
          steps {
            container('trivy') {
              sh 'trivy fs --scanners vuln,secret --severity HIGH,CRITICAL --exit-code 1 --skip-dirs target --skip-dirs reports --no-progress .'
            }
          }
        }
      }
    }

    stage('DAST & Load Test') {
      when { expression { env.SKIP_CI != 'true' } }
      stages {
        stage('Start Ephemeral Server') {
          steps {
            container('rust') {
              sh '''
                JENKINS_NODE_COOKIE=dontKillMe setsid nohup env PORT=18080 APP_VERSION="$TAG" RESTOCK_INTERVAL_SECS=0 DATABASE_URL="$PG_URL" \
                  ./target/debug/app-repo > reports/app.log 2>&1 &
                for i in $(seq 1 30); do
                  # Image rust:slim tidak punya wget/curl; pakai /dev/tcp bawaan bash.
                  bash -c 'exec 3<>/dev/tcp/127.0.0.1/18080 && printf "GET /ready HTTP/1.0\\r\\n\\r\\n" >&3 && head -1 <&3 | grep -q " 200 "' && exit 0
                  sleep 1
                done
                cat reports/app.log
                exit 1
              '''
            }
          }
        }
        stage('DAST (OWASP ZAP)') {
          steps {
            container('zap') {
              // -I: WARN tidak menggagalkan build; rule penting di .zap/rules.tsv di-set FAIL.
              sh '''
                cp .zap/rules.tsv /zap/wrk/
                status=0
                zap-baseline.py -t "$TEST_URL" -c rules.tsv -I -r zap-report.html -J zap-report.json || status=$?
                cp /zap/wrk/zap-report.* reports/ || true
                exit $status
              '''
            }
          }
        }
        stage('Load Test (k6)') {
          steps {
            container('k6') {
              sh 'k6 run -e BASE_URL="$TEST_URL" -e PROFILE=smoke --summary-export reports/k6-summary.json tests/load/stress.js'
            }
          }
        }
      }
    }

    stage('Build & Push Image') {
      when { expression { env.SKIP_CI != 'true' } }
      steps {
        container('kaniko') {
          sh '''
            /kaniko/executor \
              --context "dir://$WORKSPACE" \
              --dockerfile Dockerfile \
              --build-arg APP_VERSION="$TAG" \
              --destination "$IMAGE:$TAG" \
              --destination "$IMAGE:latest" \
              --cache=true \
              --cache-repo "$IMAGE/cache"
          '''
        }
      }
    }

    stage('Image Scan (Trivy)') {
      when { expression { env.SKIP_CI != 'true' } }
      steps {
        container('trivy') {
          sh 'trivy image --severity CRITICAL --ignore-unfixed --exit-code 1 --no-progress "$IMAGE:$TAG"'
        }
      }
    }

    stage('GitOps: Update Manifest') {
      when { expression { env.SKIP_CI != 'true' } }
      steps {
        withCredentials([usernamePassword(credentialsId: 'github-creds', usernameVariable: 'GH_USER', passwordVariable: 'GH_TOKEN')]) {
          sh '''
            sed -i "s|image: $IMAGE:.*|image: $IMAGE:$TAG|" "$MANIFEST"
            git config user.name  "Jenkins CI"
            git config user.email "jenkins@kcd-bandung.local"
            git add "$MANIFEST"
            git commit -m "deploy: ecommerce-app $TAG [skip ci]"
            git push "https://$GH_USER:$GH_TOKEN@$GIT_REPO" HEAD:main
          '''
        }
      }
    }
  }

  post {
    always {
      archiveArtifacts artifacts: 'reports/**', allowEmptyArchive: true
    }
    success {
      script {
        if (env.SKIP_CI != 'true') {
          echo "🚀 $IMAGE:$TAG lolos semua gate. ArgoCD akan men-deploy dalam hitungan detik."
        }
      }
    }
    failure {
      echo '❌ Pipeline gagal. Image tidak di-deploy; versi yang berjalan di cluster tetap aman.'
    }
  }
}
