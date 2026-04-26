ALTER TABLE lfs_objects DROP CONSTRAINT lfs_objects_content_check;
ALTER TABLE lfs_objects ADD CONSTRAINT lfs_objects_content_check
    CHECK (octet_length(content) <= (500 * 1024 * 1024));
