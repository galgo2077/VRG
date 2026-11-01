# PROMPT_IMPLEMENTACION.md — Ejecutar solo fase 1

Actúa como **coordinador de auditoría VRG**. Lee las reglas globales y `AGENTS.md`, `CONSTITUCION.md`, `DECISIONES_Y_PENDIENTES.md`, este prompt, `FASE_1.md`, `GOAL.md` y la plantilla de informe. La tarea actual consiste **exclusivamente en auditar código real y documentarlo**.

## Procedimiento operacional

1. Verifica qué raíces de código/vault están autorizadas y existen. La ubicación inicial está sin definir: intenta localizar únicamente fuentes accesibles en el ámbito del proyecto y configuración pertinente; si faltan, marca bloqueo concreto sin explorar carpetas personales indiscriminadamente.
2. Construye inventario de alto nivel sin volcar árboles completos en la conversación.
3. Asigna 8–12 áreas de auditoría independientes, tantos agentes como trabajo distinto y recursos permitan. Registra reparto y ownership. No es una cola lineal si hay tareas independientes.
4. Cada agente lee solo archivos necesarios para su área, documenta rutas/símbolos y evidencia. No ejecutar nada del producto.
5. Consolida resultados sin repetir todo el árbol, detecta solapamientos y contradicciones de evidencia.
6. Revisa si existen restricciones documentales sobre notas sensibles; nunca revelar contenidos confidenciales, tokens o claves.
7. Clasifica cada componente significativo con evidencia, no con intuición: `MANTENER`/`ADAPTAR`/`REEMPLAZAR`/`ELIMINAR`/`DESCONOCIDO`.
8. Produce un **Markdown extremadamente completo** pero sin relleno, con índice, tablas, rutas, motivos y criterios de decisión según plantilla.
9. Propón catálogo de pruebas mínimas y de controles de integridad migratoria, **sin ejecutar ni dar por aprobada ninguna**.
10. Entrega un resumen final detallado de resultados, hallazgos, bloqueos y decisiones requeridas. En mensajes intermedios usa como máximo tres líneas útiles.

## Prohibiciones expresas

No `cargo test`, no `cargo check`, no servidores, no ejecución de scripts/hooks, no depuración dinámica, no limpieza, no refactors, no modificaciones de Obsidian, no instalaciones, no migración, no pasar a fase 2. No imprimir logs ni archivos completos en chat. No inventar métricas de cobertura o rendimiento.

## Control de calidad editorial (NO test)

Comprobar visualmente durante redacción que el informe distingue hechos de propuestas y que cada conclusión relevante tiene evidencia. No ejecutar scripts de validación ni nuevas suites para verificar el documento.

## Condición de parada

Si el informe basado en fuentes inspeccionadas está completo, declarar `FASE_1_DOCUMENTADA`. Si no se pudieron ubicar los sistemas, declarar `FASE_1_BLOQUEADA_POR_ACCESO`, explicar la evidencia faltante, y detenerse. No prolongar indefinidamente la exploración.
