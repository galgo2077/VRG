# FASE_5.md — Migración final de las notas al índice RAG + grafo

## Definición estricta de «migrar todas las notas a RAG»

El usuario conserva **todos sus archivos Markdown originales de Obsidian**, legibles, navegables y editables. La migración significa **incorporar cada nota autorizada** a un sistema de índices derivados: texto, semántica, nodos/aristas y metadatos. No mover los `.md` a un formato propietario, no borrar los originales y no duplicar sus copias editables. Si el usuario posteriormente desea reorganizar físicamente archivos, es otro proyecto y requiere aprobación.

## Preconditions innegociables

- Fase 4 certificada sin bloqueadores críticos para integridad o seguridad.
- Inventario real y conjunto de vaults/rutas permitidos y excluidos, aprobados.
- Backups/manifest y espacio suficiente definidos; reversible sin tocar originales.
- Lista cerrada de verificaciones posmigración fase 5 incluida en autorización **antes de fase 3**.
- Política de escrituras automáticas con versionado y restricciones por vault.

## Flujo de migración (una sola operación coordinada)

1. Descubrir la cobertura real de cada vault autorizado desde el manifiesto de fase 1 y los cambios registrados desde entonces; no buscar fuera de las raíces permitidas.
2. Registrar para cada fuente: vault, path canonizado, nota ID estable si se aprobó, hash/revisión, tipo de metadatos, exclusiones, links y estado.
3. Indexar **en sombra** notas Markdown y fragmentos; separar relaciones explícitas de similitudes semánticas. Resolver wikilinks y backlinks hasta donde sea determinístico.
4. Ejecutar parsing e indexación incremental, deduplicar por contenido cuando proceda sin colapsar identidades distintas, y evitar re-embedding de notas sin cambios.
5. Repartir shards para procesamiento por 8–12+ agentes/worker tasks si hay suficientes datos y RAM; **un único coordinador de commits** publica versiones, checkpoints y manifests.
6. Usar journal/commit atómico de snapshots de índices. Si hay modificación concurrente de fuente, detectar revisión, reencolar nota y nunca publicar versión aparentemente correcta con fuente antigua.
7. Preparar reconciliación explícita de notas inválidas/inaccesibles, enlaces ambiguos, datos sensibles y notas excluidas; nunca fabricarlos ni ocultarlos.
8. Hacer cutover de índices solo si están disponibles las condiciones operativas de integridad autorizadas; preservar la versión anterior como rollback.
9. Ejecutar **exclusivamente las comprobaciones de migración IDs fase 5 de la lista aprobada**. Si faltan, NO llamar a la migración `VALIDADA` ni ejecutar otras por iniciativa propia.
10. Publicar un `INFORME_MIGRACION.md`: universo autorizado, cobertura, exclusiones, errores, fuentes originales, índices publicados, revisión, recuperación y pendientes.

## Manejo de casos difíciles

| Caso | Política obligatoria |
|---|---|
| Nota corrupta o frontmatter inválido | conservar fuente; marcar nota rechazada/parcial; nunca reescribir silenciosamente |
| Symlink fuera de raíz permitida | no seguir, bloquear como fuera de alcance |
| Rename mientras se indexa | identidad estable/cambio de revisión; reencolar, sin duplicar nota |
| Notas duplicadas con contenido idéntico | no confundir identidad con contenido; deduplicar embedding si es seguro |
| Enlace ambiguo | preservar ambigüedad explícita, no crear relación falsa |
| Embeddings locales indisponibles | marcar falta semántica; índice léxico/grafo solo si modo degradado autorizado |
| Espacio insuficiente/IO fallido | abortar commit, mantener índice anterior y originales intactos |
| Cortes de energía/reinicio | reanudar desde journal válido o revertir snapshot derivado |
| Fuente cambia después del snapshot | versión de índice caduca; reindexación incremental versionada |
| Escritura automática a nota | usar revision guard, journal, operación atómica y rollback; nunca escritura ciega |
| Interfaz ChatGPT no disponible | fase 5 puede completar indexación local, informar conexión remota pendiente |

## Qué significa «todas»

Todas las notas Markdown **dentro de las raíces y filtros autorizados**, incluyendo notas sin enlaces. No implica indexar secretos, copias no autorizadas, repositorios ajenos, basureros, backups o enlaces externos. Informar denominador, excluidas, pendientes y errores; no anunciar 100 % si hay notas sin indexar.

## Resultados y parada

- `INFORME_MIGRACION.md` con recuentos reales y cobertura por vault.
- Índice RAG/grafo local publicado o estado de fallo reversible.
- Notas originales intactas y reversión de índices disponible.
- Solo pruebas de migración expresamente autorizadas ejecutadas al final.

Detenerse al finalizar la migración aprobada o al encontrar bloqueo serio; no introducir nuevas tareas, modelos, sincronizadores o pruebas.
