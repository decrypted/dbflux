# Trabajar con resultados

Los resultados se renderizan en pestañas de resultado dentro del documento. Cada
tabla o colección se abre en la vista que su driver declara para ese objeto, y
ofrece las demás vistas que declara:

- **Vista de tabla** para tablas relacionales y resultados de query.
- **Vista de árbol** para colecciones de documentos (por ejemplo MongoDB,
  DynamoDB), con vistas de **Tabla** y **JSON** de la misma página (ver
  [Colecciones de documentos](DOCUMENTS.md)).
- **Vista de tabla** con una vista de **Árbol** de las mismas filas para una
  tabla con columnas JSON (ver
  [Columnas JSON en una tabla](DOCUMENTS.md#columnas-json-en-una-tabla)).
- **Gráfico sobre la cuadrícula** para una serie temporal, como un measurement
  de InfluxDB o una hypertable de TimescaleDB, cuando sus filas se pueden
  graficar (ver
  [Tablas y colecciones de series temporales](CHARTS.md#tablas-y-colecciones-de-series-temporales)).
- **Vista clave-valor** para Redis (ver [Vista clave-valor](KEY_VALUE.md)).

Una tabla con una columna vectorial también lista las filas más
[similares](#filas-similares) a la seleccionada.

Los contenedores de tipo event-stream se abren como event streams cuando el
driver declara esa presentación.

## Resultados junto al editor

En una pestaña de consulta los resultados están debajo del editor. En una
pantalla ancha, muévelos a la derecha del editor con el botón de panel de la
cabecera de resultados, la entrada **Mostrar los resultados a la derecha** de
las acciones del panel o **Mover los resultados junto al editor o debajo** en la
paleta de comandos. El mismo control los vuelve a poner debajo. Arrastra el
divisor para cambiar el tamaño de cada lado. Las pestañas de consulta nuevas se
abren con la última posición elegida.

En cualquiera de las dos posiciones, `Ctrl+j` lleva el teclado del editor a los
resultados y `Ctrl+k` lo devuelve.

## Navegar el data grid

Cuando el panel de resultados tiene el foco:

- `j`/`k` (o `Down`/`Up`) — moverse entre filas.
- `h`/`l` (o `Left`/`Right`) — moverse entre columnas.
- `g`/`Shift+g` (o `Home`/`End`) — primera / última fila.
- `Ctrl+d`/`Ctrl+u` (o `PageDown`/`PageUp`) — recorrer filas por páginas.
- `[` / `]` — página anterior / siguiente de resultados (paginación).
- `f` enfoca la toolbar; `/` enfoca la búsqueda/filtro.
- `z` alterna el colapso del panel.
- `m` (o `Shift+F10`) abre el menú contextual de fila/celda.

Con el ratón, un clic selecciona una celda y `Shift`+clic, o pulsar y arrastrar, selecciona el rango hasta otra celda. Un clic en un número de fila selecciona la fila entera, y `Shift`+clic en otro número de fila selecciona todas las filas intermedias. `Ctrl`+clic (`Cmd`+clic en macOS) en una celda o un número de fila la añade a la selección, o la quita cuando está seleccionada sola; al copiar varios rangos se copian las filas y columnas que abarcan, con las celdas fuera de la selección vacías. Al arrastrar más allá de un borde de la cuadrícula, esta se desplaza en esa dirección, más deprisa cuanto más lejos esté el puntero, y el rango sigue creciendo hasta soltar el botón o volver con el puntero.

Cuando la selección abarca más de una celda, la barra de estado muestra cuántas celdas contiene y, para los números entre ellas, su suma, media, mínimo y máximo. Los NULL y el texto se cuentan pero quedan fuera de las cifras, y la barra de estado indica cuántos ha omitido. Los totales se calculan con las filas ya cargadas, incluidas las ediciones sin guardar, sin volver a consultar la base de datos.

## Vista de registro

Pulsa `i`, o usa el conmutador Registro en la barra de estado del
resultado, para mostrar la fila activa como una lista de Nombre / Valor que
ocupa toda el área de resultados. La cabecera indica la posición de la fila en
el resultado. Los campos se editan igual que las celdas de la cuadrícula, así
que los cambios sin guardar, Guardar fila y revertir funcionan igual en ambos
modos; `Up`/`Down` recorren los campos y `Left`/`Right` cambian de fila. Pulsa
`i` de nuevo para volver a la cuadrícula.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/results/record-view-dark.webp">
  <img src="../images/results/record-view-light.webp" alt="La vista de registro con una fila de cliente como lista de campos y valores">
</picture>

## Menú de la cabecera de columna

Haz clic derecho en la cabecera de una columna para abrir un menú limitado a
esa columna: ordenar ascendente o descendente, quitar el orden y todos los
operadores de filtro, en una sola lista plana. El clic izquierdo en la
cabecera selecciona la columna entera (`Shift`+clic selecciona todas las columnas hasta ella, `Ctrl`+clic la añade o la quita); la flecha en el borde derecho de la cabecera alterna el orden, y muestra una flecha arriba/abajo tenue mientras la columna no está ordenada.

Para ordenar por varias columnas, haz `Alt`+clic en la flecha de otra columna:
pasa a ser la siguiente clave de orden, ascendente, y otro `Alt`+clic la vuelve
descendente y después la quita del orden. Con más de una clave, cada cabecera
ordenada muestra su posición (1, 2, 3) junto a la flecha, y las filas que
empatan en la primera clave se ordenan por la segunda, y así sucesivamente. Un
clic normal en cualquier flecha vuelve a ordenar solo por esa columna. El menú
de la columna ofrece lo mismo con **Añadir al orden: columna ASC** y **Añadir
al orden: columna DESC**, que también cambian la dirección de una columna que ya
es clave, y **Quitar orden** elimina todas las claves. Una tabla envía todas las
claves a la base de datos en un único `ORDER BY`; un resultado de consulta se
ordena en memoria. En una tabla que sigue en su orden por defecto (su clave
primaria), la primera clave añadida sustituye ese orden, porque las filas
ordenadas primero por la clave primaria no dejan empates que otra columna pueda
ordenar; quitar el orden vuelve al orden por defecto.

`Shift+m` abre el mismo menú desde el teclado, para la columna de la celda
seleccionada. El menú termina con una sección **Columna** de ajustes de vista,
que cambian cómo la grilla muestra la columna y nunca la consulta. **Ajustar
ancho** ajusta la columna a su cabecera y a sus valores cargados, hasta 640
píxeles para que un valor JSON largo no saque el resto de la grilla de la
pantalla; un doble clic en el borde de redimensionado, a la derecha de la
cabecera, hace lo mismo.

**Ocultar columna** quita la columna de la pantalla, y **Columnas…** lista todas
las columnas con una marca en las visibles: `Enter`, `Space` o un clic muestra u
oculta la columna resaltada mientras la lista sigue abierta, **Mostrar todas las
columnas** devuelve todas las ocultas, y `Escape` la cierra. **Mover a la
izquierda** y **Mover a la derecha** mueven la columna un lugar; arrastrar una
cabecera sobre otra mueve su columna a ese lugar.

**Fijar columna** mantiene la columna en el borde izquierdo, justo después de
los números de fila, mientras la grilla se desplaza en horizontal, como un panel
inmovilizado: fija `id` y `created_at` para tenerlas a la vista en una tabla de
logs ancha. Las columnas fijadas van al principio de la grilla en el orden en
que se fijaron y muestran un alfiler en su cabecera; **Soltar columna** deja que
la columna vuelva a desplazarse, como la primera de las demás. Seleccionar,
editar, ordenar, redimensionar y los números de fila funcionan igual en las
columnas fijadas. Mover a la izquierda y Mover a la derecha se quedan en su lado
del borde de las fijadas; arrastrar una cabecera a través de él fija o suelta su
columna.

La consulta sigue leyendo todas las columnas (una tabla sigue ejecutando
`SELECT *`); para dejar una columna fuera de la consulta misma, usa la
proyección del constructor de consultas. Copiar (con o sin nombres de columna),
Copiar como CSV, Copiar como JSON, Copiar como lista IN de SQL, Copiar como
sentencias INSERT y Copiar como tabla Markdown siguen la pantalla: las columnas
visibles, en el orden en que la grilla las muestra, y la lista IN toma la que
está más a la izquierda. Copiar fila, Copiar como consulta y Exportar usan la fila completa en
el orden propio del resultado, columnas ocultas incluidas. Una tabla recuerda su
disposición para su conexión, base de datos, esquema y tabla, al cerrarla y al
reiniciar DBFlux; volver a poner todas las columnas en el orden propio de la
tabla, todas visibles, la olvida. Un resultado de consulta conserva su
disposición mientras su pestaña está abierta. Una grilla de documentos mantiene
el orden de sus campos y solo ofrece Ajustar ancho.

## Panel de valor

Haz clic derecho en una celda y elige **Ver valor**, o pulsa `v`, para abrir la
celda en el panel inspector de la derecha. Muestra el valor como JSON, XML o
texto plano — detectado a partir del contenido, y solo cuando realmente se
analiza — con formato legible, compacto y ajuste de línea. Se puede editar allí:
**Guardar** confirma la fila directamente y **Revertir** descarta el cambio. El
panel sigue la celda seleccionada al moverte por la cuadrícula, salvo mientras
tenga un cambio sin guardar.

## Inspector de fila

Pulsa `Ctrl+Space`, o haz clic derecho en una fila y elige **Inspeccionar
fila**, para abrir la fila seleccionada en el panel inspector de la derecha.
Lista cada columna de la fila con su valor y marca las columnas de clave
primaria y foránea. El inspector sigue la fila seleccionada al moverte por la
cuadrícula; el botón de fijar de su cabecera lo mantiene en la fila actual.
**Editar**, **Duplicar** y **Eliminar**, al pie, actúan sobre la fila
inspeccionada cuando el resultado es editable. Pulsa `Ctrl+Space` de nuevo, o
el botón de cerrar, para ocultarlo.

Bajo **Referencias** el inspector lista las claves foráneas de la fila en ambas
direcciones:

- **Salientes** — una clave foránea de una sola columna de esta fila apunta a
  otra tabla. La entrada nombra esa tabla y la fila a la que apunta, por su
  valor legible una vez resuelto («María López · #8»), o por su clave hasta
  entonces. Haz clic en ella para abrir la tabla referenciada en esa fila:
  DBFlux reutiliza la pestaña existente de la tabla — abriendo una si no la
  hay — y reemplaza su filtro por una igualdad de clave primaria, de modo que
  la fila referenciada sea la visible.
- **Entrantes** — otras tablas tienen claves foráneas que apuntan a esta fila.
  Cada entrada nombra la tabla y cuenta cuántas de sus filas la referencian;
  los conteos se cargan en segundo plano. Estas entradas son solo
  informativas.

Las celdas de clave foránea de la cuadrícula pueden mostrar el mismo valor del
registro referenciado junto a su id: activa **Mostrar el valor del registro
referenciado en celdas de clave foránea** en [Configuración](SETTINGS.md). El
inspector de fila resuelve las referencias independientemente de ese ajuste.

## Filtrar resultados

La toolbar del data grid tiene un input de filtro `WHERE` que vuelve a ejecutar
la query con la condición que escribas. Para conexiones SQL soporta dos estilos:

- **`WHERE` en bruto** — escribe una condición plana (por ejemplo `status =
  'active'`). Este es el comportamiento por defecto.
- **Rutas relacionales (estilo ORM)** — escribe una ruta con puntos que recorre
  foreign keys, por ejemplo `created_by.email LIKE '%@acme.com'` o
  `created_by.organization.name = 'Acme'`. DBFlux resuelve la ruta contra los
  metadatos de foreign key de la tabla y hace los joins hasta la tabla
  referenciada por ti; no hace falta escribir los JOINs a mano.

Cuando un filtro relacional se resuelve, un chip muestra cuántos joins añadió.
Si un segmento es ambiguo o no se puede resolver, aparece un error inline con un
enlace **Open in builder** que abre el constructor visual de queries precargado
con los joins resueltos hasta ese punto. La entrada sin puntos siempre mantiene
el comportamiento de `WHERE` en bruto.

El input de filtro también ofrece autocompletado consciente del schema (misma
navegación que el builder — ver [Autocompletado consciente del
schema](QUERY_BUILDER.md#autocompletado-consciente-del-schema)).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/results/filtering-dark.webp">
  <img src="../images/results/filtering-light.webp" alt="La tabla customers filtrada con una condición WHERE escrita en la barra de filtro">
</picture>

## Filas similares

En una tabla con una columna vectorial (hoy, una tabla PostgreSQL con una
columna `vector` de pgvector), pulsa `Shift+s` o elige **Buscar filas
similares** en el menú de la fila para listar las filas más cercanas a la fila
seleccionada. La entrada del menú indica qué columna lee y cuántas filas lista,
o por qué no puede ejecutarse: la fila no tiene valor en esa columna, o la
cuadrícula tiene cambios sin guardar, que hay que guardar o revertir antes.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/results/similar-rows-dark.webp">
  <img src="../images/results/similar-rows-light.webp" alt="La tabla products tras Buscar filas similares: la fila seleccionada fijada arriba como ancla, sus diez filas más cercanas ordenadas por distancia L2 y la barra con la columna vectorial, la distancia y K">
</picture>

La cuadrícula muestra entonces la fila seleccionada fijada arriba, marcada como
`referencia`, y debajo sus 10 filas más cercanas, de la más cercana a la más
lejana. La primera columna contiene la distancia, bajo una cabecera que la
nombra. Una barra reemplaza la fila de filtro:

- El selector de columna elige la columna vectorial. Con el cursor sobre una
  columna vectorial, la búsqueda empieza en esa columna; si no, en la primera de
  la tabla.
- **L2**, **Coseno** y **Producto interno** cambian la distancia. Producto
  interno muestra el producto en sí, de mayor a menor.
- **K** fija cuántas filas listar, de 1 a 1000.
- **Volver a la tabla**, o `Esc`, regresa a la página de la tabla con su filtro.

La fila seleccionada queda fuera de sus vecinas por su clave primaria, las filas
sin vector no se listan y las filas similares son de solo lectura. Una distancia
que no es un número se muestra como un guion. Las columnas `halfvec` y
`sparsevec` no ofrecen la búsqueda.

## Editar y CRUD

En el data grid:

- `o` — añadir una fila.
- `x` — eliminar la fila seleccionada.
- `r` — renombrar / editar (según el contexto).
- `y` — copiar la fila seleccionada.
- `Ctrl+c` (`Cmd+c`) — copiar la(s) celda(s) seleccionada(s) al portapapeles.

Con varias celdas seleccionadas, `Enter`, `F2` o un doble clic en una de ellas abre el editor para todas, y la barra de estado indica cuántas celdas cambiará la edición. El valor que confirmas se escribe en cada celda seleccionada que lo admita, como cambios sin guardar que Guardar y Revertir tratan igual que cualquier otra edición, y un solo deshacer (`u` o `Ctrl+z`) revierte la edición entera. Se omiten las celdas de columnas de solo lectura, las filas marcadas para eliminar y los valores que el editor en línea no edita (JSON, texto largo o de varias líneas); un valor elegido de la lista de valores permitidos de una columna solo va a columnas con la misma lista.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/results/editing-dark.webp">
  <img src="../images/results/editing-light.webp" alt="La tabla customers con una celda editada aún sin guardar y los botones Save y Revert habilitados">
</picture>

### Cuándo los resultados son editables

Los browses de tabla planos son editables cuando la tabla tiene una primary key.
Los resultados producidos por el **constructor visual de queries** (modo SELECT)
también son editables, pero solo cuando están demostrablemente vinculados a una
única tabla: el resultado mapea 1:1 a una tabla subyacente y cada columna de
primary key de esa tabla está proyectada con su nombre original. Las ediciones y
eliminaciones construyen entonces su `WHERE` a partir de los valores de primary
key proyectados.

Los JOINs están permitidos: las columnas de la tabla origen son editables,
mientras que las columnas unidas son de solo lectura.

Un resultado del builder recae en **solo lectura** — con una pista en la toolbar
explicando por qué — cuando se cumple alguna de estas condiciones:

- La query agrega o usa `GROUP BY` / `HAVING`.
- La proyección es un wildcard a través de un JOIN.
- Falta una columna de primary key o está proyectada bajo un alias.
- Las keys de la tabla aún no se han cargado desde la caché del schema (el grid
  se actualiza a editable en cuanto llegan las keys).

El SQL de forma libre escrito en el editor sigue siendo de solo lectura; la
edición inline solo aplica a browses de tabla planos y a SELECTs generados por
el builder.

### Resultados agregados

Cuando un resultado proviene de una query agrupada (`GROUP BY`), las filas
muestran la salida agregada y la edición está deshabilitada — añadir fila,
eliminar fila, editar celda e inspeccionar fila no están disponibles, con
tooltips explicativos. La paginación cuenta las filas agrupadas (no las filas
subyacentes), así que el total de páginas es correcto. Las columnas de agregado
conservan el tipo de columna correcto, así que graficar sigue funcionando.

## Copiar como CSV o JSON

El menú contextual de un resultado de tabla tiene **Copiar como CSV** y
**Copiar como JSON** debajo de **Copiar**. Copian la selección, sea un rango o
varios elegidos con `Ctrl`+clic, con el mismo exportador que **Copiar filas
cargadas al portapapeles**: el CSV empieza con una fila de cabecera con los
nombres de las columnas, y el JSON es un array con un objeto por fila, con las
columnas como claves y los números, booleanos y `null` como tales. Se incluyen
todas las filas y columnas que abarca la selección; una celda dentro de esa área
pero fuera de la selección queda vacía en CSV y `null` en JSON. **Copiar**
(`Ctrl+c`) copia texto separado por tabulaciones, sin cabecera salvo que
**Incluir los nombres de columna al copiar** esté activado en Ajustes → General
→ Cuadrícula de datos, que lo empieza con una línea con los nombres de las
columnas copiadas.
`Ctrl+Alt+c` (`Cmd+Alt+c` en macOS) ejecuta **Copiar como JSON** desde el
teclado; el menú lo muestra junto a la entrada.

Debajo, el menú tiene tres extractores más:

- **Copiar como lista IN de SQL** copia los valores seleccionados de una
  columna como lista para un `WHERE ... IN`, por ejemplo `('a', 'O''Brien')` o
  `(1, 2, 3)`, con las comillas que usa el driver de la conexión (comillas ANSI
  en un resultado sin conexión). Un valor seleccionado varias veces aparece una
  sola vez y `NULL` se omite, porque `IN (NULL)` no coincide con nada. Con
  varias columnas seleccionadas usa la de más a la izquierda.
- **Copiar como sentencias INSERT**, en una tabla, copia un `INSERT` por cada
  fila seleccionada con las columnas seleccionadas, escrito por el mismo
  generador del driver que **Copiar como query**. Cada fila aporta sus propios
  valores para esas columnas, aunque una celda dentro del área no esté
  seleccionada.
- **Copiar como tabla Markdown** copia las mismas filas y columnas que Copiar
  como CSV como una tabla Markdown con fila de cabecera; las celdas `NULL` y
  las no seleccionadas quedan vacías, y `|` y los saltos de línea dentro de los
  valores se escapan.

## Copiar como query

El menú contextual de resultados incluye **Copy as Query**, que genera una
sentencia de mutación específica del driver (o un envelope, para drivers no SQL)
a partir de la fila seleccionada usando el generador de queries propio del
driver.

## Exportar

Pulsa `Ctrl+e` (`Cmd+e`) en el panel de resultados, o ejecuta **Export results**
desde el command palette. Los formatos disponibles dependen de la forma del
resultado e incluyen:

- **CSV**
- **JSON (pretty)** y **JSON (compact)**
- **Text**
- **Binary** (para resultados con forma binaria)

Las secciones **Guardar filas cargadas como archivo** y **Copiar filas cargadas
al portapapeles** del menú escriben las filas que tiene la cuadrícula, que en una
tabla son la página actual, y el aviso indica cuántas filas fueron al archivo o
al portapapeles.

Cuando la cuadrícula muestra una tabla o una colección leída página a página, el
menú añade **Exportar todas las filas a un archivo**, en CSV o en JSON con un
objeto por línea. Vuelve a leer la tabla o la colección completa con el filtro,
el orden y la selección de columnas actuales de la cuadrícula, una página a la
vez, y la escribe en el único archivo que elijas, de modo que los datos nunca
tienen que caber en memoria. Una tabla mostrada mediante el constructor de
consultas o un filtro relacional se lee con esa misma consulta, desde su
desplazamiento y hasta su límite cuando lo define. La exportación se ejecuta
como una tarea en el panel de tareas, que muestra su progreso y permite
cancelarla; el aviso indica el total de filas y la ruta.

Una colección de documentos exportada como JSON escribe cada documento
completo, un objeto por línea, con todos sus campos. Como CSV se escribe con
las columnas que la cuadrícula muestra para la primera página, incluidos los
objetos expandidos. Los documentos de una colección no tienen por qué
compartir sus campos, así que en un CSV un campo que aparece por primera vez en
una página posterior no tiene columna, porque el encabezado ya está escrito; en
ese caso la exportación nombra esos campos en una advertencia.

Un resultado de consulta ofrece solo las filas cargadas, porque exportarlas
todas implicaría volver a ejecutar la consulta. Lo mismo ocurre con una búsqueda
de vecinos más cercanos: devuelve un número fijo de las filas más próximas, y la
cuadrícula ya las tiene todas.
