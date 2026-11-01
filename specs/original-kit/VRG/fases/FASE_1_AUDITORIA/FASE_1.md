# FASE_1.md — Auditoría exhaustiva de infraestructura y código reales

## Objetivo

Comprender el estado actual real de toda la memoria: vaults Obsidian, scripts, hooks, sincronización, backups, Codex, MCP y conexiones externas. Producir un único informe Markdown profundo, verificable y útil para rediseñar VRG. No modificar producto ni ejecutar tests.

## Entradas

- `CONSTITUCION.md`, `DECISIONES_Y_PENDIENTES.md` y especificaciones existentes.
- Repositorios y vaults **efectivamente accesibles y autorizados**. Sus rutas no están definidas de antemano.
- Configuración local observable sin revelar secretos.

## Límites

- Solo inspección estática; lectura de archivos y metadatos. No ejecutar scripts, hooks, servidores, agentes, test suites, benchmarks, compilaciones ni migraciones.
- Ninguna búsqueda recursiva indiscriminada de archivos personales: primero identificar raíces permitidas.
- No instalar dependencias, modificar global config, cambiar permisos ni crear symlinks.
- Si falta una raíz, describirla `NO_INSPECCIONADA` y el bloqueo concreto. No inventar arquitectura.
- Nunca declarar error funcional comprobado por pruebas; usar `riesgo o defecto inferido por auditoría estática`.

## Frentes paralelos

El coordinador identifica zonas reales y reparte 8–12 agentes por temas sin relecturas duplicadas: 1) estructura de repositorios; 2) notas/Markdown; 3) wikilinks/grafo; 4) scripts y hooks; 5) sincronización y backups; 6) Codex y AGENTS; 7) MCP y conexión ChatGPT; 8) seguridad/permisos; 9) historiales/errores; 10) dependencias/almacenamiento; adicionales solo si hay frentes nuevos independientes. Los agentes entregan mapas y matrices, nunca evaluaciones especulativas.

## Investigaciones obligatorias

1. Catalogar cada subsistema significativo con ruta real, entrada/salida, dependencias, grado de uso y propietario.
2. Rastrear cómo se crean, editan, mueven, renombran y sincronizan las notas.
3. Distinguir información fuente de índices/caches reconstruibles, copias, mirrors y backups.
4. Reconocer los enlaces y metadatos Obsidian sin transformarlos ni reescribirlos.
5. Identificar mecanismos existentes de memoria/recuperación y qué ya resuelve VRG.
6. Identificar riesgos de symlinks, path traversal, secrets, autorización y ediciones concurrentes.
7. Identificar qué acciones de ingeniería pueden borrar datos y dónde debe bloquearse la fase 2.
8. Mapear capacidades reales de Codex/MCP y la diferencia entre MCP local y ChatGPT remoto.
9. Evaluar cada módulo con `MANTENER`, `ADAPTAR`, `REEMPLAZAR`, `ELIMINAR`, `DESCONOCIDO` y evidencia.
10. Preparar lista de componentes **candidatos**, no autorización de borrado.
11. Identificar contratos y módulos Rust futuros solo como propuesta sujeta a aprobación.
12. Proponer lista MÍNIMA de pruebas fase 4 y controles de migración fase 5, con IDs, razones y criterios; **NO EJECUTAR**.
13. Señalar rutas que no deben tocarse y límites de recursos observables.
14. Documentar cómo recuperar fallos de cada área como requisitos futuros.
15. Entregar cobertura de auditoría y preguntas sin resolver, diferenciando hechos e hipótesis.

## Entregables

- `INFORME_AUDITORIA_REAL.md` EXTENSO en el directorio de fase 1, generado desde código real, siguiendo `plantillas/INFORME_AUDITORIA_REAL_TEMPLATE.md`.
- Matriz por archivo/componente de clasificación y evidencia trazable.
- Propuesta detallada de acciones fase 2, arquitectura fase 3 y pruebas cerradas fases 4–5.
- Resumen muy breve de avances en chat; reporte final completo con alcance y bloqueos.

## Cierre y transición

Detenerse tras entregar el informe. El usuario lo compartirá para definir el rediseño y aprobar qué se elimina y qué pruebas se ejecutan. **Prohibido iniciar fase 2 o escribir módulos automáticamente.**
