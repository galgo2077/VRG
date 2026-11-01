# AGENTS.md — instrucciones locales de VRG para Codex

**Regla de lectura:** No reemplaza reglas globales `~/.codex/AGENTS.md`. Complementa esas reglas dentro del repositorio VRG. Lee `CONSTITUCION.md`, `DECISIONES_Y_PENDIENTES.md`, la especificación original y solamente los 3 documentos de la fase activa. Conserva referencias a ellos durante toda la tarea.

1. Fase activa **una sola**: no adelantar limpiezas, pruebas o migración.
2. Separar hechos observados, hipótesis, recomendaciones y permisos; no inventar código ni rutas.
3. Antes de cambiar cualquier archivo, confirmar alcance y propiedad del agente.
4. Emplear 8–12+ agentes independientes cuando existan tareas no solapadas y recursos; no forzar agentes ociosos. Un coordinador conserva decisiones, dependencias y rutas.
5. Paralelizar lecturas y módulos sin escritura compartida. Pasar resultados estructurados compactos.
6. No emitir logs de terminal en el chat; mostrar decisiones y resúmenes. Guardar evidencia útil en el informe de fase 1 con enlaces y fragmentos breves, no volcados íntegros.
7. No volver a leer archivos ya inspeccionados si no cambiaron o no apareció una duda nueva.
8. No hacer microtests. Cumplir la matriz de pruebas autorizadas. Cualquier `cargo check` de fase 3 sucede tras completar todo el código planificado.
9. Si fase 1 detecta hechos que invalidan hipótesis, registrarlos; no reescribir requisitos sin autorización.
10. Protección especial: nunca borrar notas originales, índices históricos, respaldos, repositorios de origen o configuración global por iniciativa propia.
11. Si el GOAL queda cumplido, detenerse de inmediato y entregar el informe final; no autoabrir nuevas tareas.
12. Las tareas pendientes no autorizan avanzar automáticamente de fase.
13. El informe final incluye cambios, evidencias, límites, pruebas autorizadas realizadas/no realizadas y bloqueos; avances durante el proceso mínimos.

**Ruta de inicio:** `fases/FASE_1_AUDITORIA/FASE_1.md`.
**Ruta del documento rector de pruebas:** `plantillas/REGISTRO_PRUEBAS_CERRADO.md`; es plantilla y **no autoriza ninguna prueba** hasta que el usuario apruebe entradas concretas.
