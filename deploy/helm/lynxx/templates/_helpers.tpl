{{/*
Common helpers for the Lynxx chart.
*/}}

{{- define "lynxx.name" -}}
{{- default "lynxx" .Chart.Name -}}
{{- end -}}

{{- define "lynxx.fullname" -}}
{{- printf "%s" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "lynxx.gatewayName" -}}
{{- printf "%s-gateway" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "lynxx.controllerName" -}}
{{- printf "%s-controller" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "lynxx.postgresName" -}}
{{- printf "%s-postgres" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "lynxx.secretName" -}}
{{- printf "%s-db" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "lynxx.configMapName" -}}
{{- printf "%s-config" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "lynxx.labels" -}}
app.kubernetes.io/name: {{ include "lynxx.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "lynxx.databaseUrl" -}}
{{- printf "postgres://%s:%s@%s:5432/%s" .Values.postgres.credentials.user .Values.postgres.credentials.password (include "lynxx.postgresName" .) .Values.postgres.credentials.database -}}
{{- end -}}
