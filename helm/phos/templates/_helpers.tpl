{{/*
Expand the name of the chart.
*/}}
{{- define "phos.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "phos.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart label.
*/}}
{{- define "phos.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels.
*/}}
{{- define "phos.labels" -}}
helm.sh/chart: {{ include "phos.chart" . }}
{{ include "phos.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels.
*/}}
{{- define "phos.selectorLabels" -}}
app.kubernetes.io/name: {{ include "phos.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Name of the Secret holding OIDC credentials
(client_id / client_secret / mobile_client_id).
*/}}
{{- define "phos.oidcSecretName" -}}
{{- if .Values.oidc.existingSecret }}
{{- .Values.oidc.existingSecret }}
{{- else }}
{{- printf "%s-zitadel" (include "phos.fullname" .) }}
{{- end }}
{{- end }}

{{/*
Name of the data PVC.
*/}}
{{- define "phos.dataClaimName" -}}
{{- if .Values.persistence.existingClaim }}
{{- .Values.persistence.existingClaim }}
{{- else }}
{{- printf "%s-data" (include "phos.fullname" .) }}
{{- end }}
{{- end }}

{{/*
Effective OIDC redirect URI.
*/}}
{{- define "phos.oidcRedirectUri" -}}
{{- if .Values.oidc.redirectUri }}
{{- .Values.oidc.redirectUri }}
{{- else }}
{{- printf "https://%s/api/auth/callback" .Values.ingress.host }}
{{- end }}
{{- end }}
