// NusaMart CI/DevSecOps pipeline
// Lint & scan → build image (Kaniko) → scan image (Trivy) → update manifest k8s/ → ArgoCD sync.
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
  containers:
    - name: rust
      image: rust:1.98-slim-trixie
      command: ["sleep"]
      args: ["infinity"]
    - name: kaniko
      image: gcr.io/kaniko-project/executor:v1.24.0-debug
      command: ["sleep"]
      args: ["infinity"]
      volumeMounts:
        - name: docker-config
          mountPath: /kaniko/.docker
    - name: trivy
      image: aquasec/trivy:0.74.0
      command: ["sleep"]
      args: ["infinity"]
      env:
        - name: DOCKER_CONFIG
          value: /kaniko/.docker
      volumeMounts:
        - name: docker-config
          mountPath: /kaniko/.docker
  volumes:
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
  }

  stages {
    stage('Prepare') {
      steps {
        script {
          // Commit manifest dari Jenkins sendiri tidak perlu dibangun ulang (mencegah loop).
          def message = sh(script: 'git log -1 --pretty=%B', returnStdout: true).trim()
          env.SKIP_CI = message.contains('[skip ci]') ? 'true' : 'false'
          env.TAG = "${env.BUILD_NUMBER}-${env.GIT_COMMIT.take(7)}"
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
        stage('Lint (fmt + clippy)') {
          steps {
            container('rust') {
              sh '''
                rustup component add rustfmt clippy
                cargo fmt --check
                cargo clippy --locked -- -D warnings
              '''
            }
          }
        }
        stage('Secret & Dependency Scan') {
          steps {
            container('trivy') {
              sh 'trivy fs --scanners vuln,secret --severity HIGH,CRITICAL --exit-code 1 --skip-dirs target --no-progress .'
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
