# VRG — Presentación pública

**VRG (Vault RAG Graph)** es un proyecto en Rust para recuperar conocimiento de vaults Markdown, preservando las notas originales y sus permisos.

## Objetivo

Permitir que Codex recupere contexto técnico relevante sin releer proyectos enteros ni almacenar conversaciones completas.

## Flujo previsto

1. El usuario autoriza raíces y exclusiones concretas.
2. VRG valida las rutas antes de leer Markdown permitido.
3. Los índices derivados gestionan búsqueda textual, vectores semánticos opcionales, revisiones y relaciones.
4. Un adaptador MCP de solo lectura proporciona búsqueda y navegación a Codex.
5. Codex puede actualizar documentación compacta en archivos autorizados; VRG indexa incrementalmente esos cambios.

## Compatibilidad con asistentes

Por ahora, **Codex es el único asistente con integración oficialmente soportada por VRG**. El CLI local puede utilizarse sin Codex. No hay soporte oficial validado para otros clientes MCP.

## Apoyo voluntario

[Buy Me a Coffee — galgo2077](https://buymeacoffee.com/galgo2077?new=1) es un sitio externo para donaciones voluntarias dirigidas al mantenimiento, documentación y desarrollo de VRG. El proyecto sigue siendo gratuito y de código abierto; no es necesario donar.

## Privacidad

Indexar localmente no significa autorizar el envío de extractos a un modelo remoto. Tampoco supone permiso para leer carpetas no autorizadas, guardar conversaciones o modificar notas originales.

## Estado público

Esta distribución incluye código y documentación de instalación. La validación registrada corresponde a casos y entorno locales; no certifica otras instalaciones.

Consulta [Inicio](GETTING_STARTED.md), [Seguridad](SECURITY.md) y [Hoja de ruta](ROADMAP.md).
